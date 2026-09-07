use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, DeviceId, SampleFormat, Stream, StreamConfig,
};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use rubato::{FftFixedIn, Resampler};
use tauri::{AppHandle, Emitter};

use crate::{
    error::{FlowError, Result},
    models::{MessagePayload, Microphone, WaveformPayload},
    platform::TargetWindow,
    recovery::RecoverySpool,
};

#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub wav: Vec<u8>,
    pub duration_ms: i64,
    pub target: TargetWindow,
    pub partial: bool,
    pub limit_reached: bool,
}

struct EncodedCapture {
    wav: Vec<u8>,
    sample_count: usize,
    audible: bool,
    partial: bool,
    limit_reached: bool,
}

struct ActiveRecording {
    session_id: u64,
    stream: Option<Stream>,
    capture_thread: Option<std::thread::JoinHandle<EncodedCapture>>,
    stop_capture: Arc<AtomicBool>,
    target: TargetWindow,
}

impl Drop for ActiveRecording {
    fn drop(&mut self) {
        self.stream.take();
        self.stop_capture.store(true, Ordering::Release);
        if let Some(thread) = self.capture_thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct AudioRecorder {
    sender: mpsc::Sender<RecorderCommand>,
    recording: Arc<AtomicBool>,
}

struct StartParams {
    session_id: u64,
    app: AppHandle,
    microphone_id: String,
    target: TargetWindow,
    spool: Option<RecoverySpool>,
    reply: mpsc::SyncSender<Result<()>>,
}

enum RecorderCommand {
    Start(Box<StartParams>),
    Stop {
        session_id: u64,
        reply: mpsc::SyncSender<Result<CapturedAudio>>,
    },
    Cancel {
        session_id: u64,
        reply: mpsc::SyncSender<Result<()>>,
    },
    StartMicTest {
        session_id: u64,
        app: AppHandle,
        microphone_id: String,
        reply: mpsc::SyncSender<Result<()>>,
    },
    StopMicTest {
        session_id: u64,
        reply: mpsc::SyncSender<Result<()>>,
    },
    CaptureLimitReached {
        session_id: u64,
        app: AppHandle,
    },
    StreamFailed {
        session_id: u64,
        app: AppHandle,
        message: String,
    },
}

impl AudioRecorder {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let recording = Arc::new(AtomicBool::new(false));
        let worker_recording = recording.clone();
        let error_sender = sender.clone();
        std::thread::Builder::new()
            .name("flow-audio".into())
            .spawn(move || recorder_worker(receiver, worker_recording, error_sender))
            .expect("could not start Flow audio worker");
        Self { sender, recording }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    pub fn start(
        &self,
        session_id: u64,
        app: AppHandle,
        microphone_id: &str,
        target: TargetWindow,
        spool: Option<RecoverySpool>,
    ) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Start(Box::new(StartParams {
                session_id,
                app,
                microphone_id: microphone_id.into(),
                target,
                spool,
                reply,
            })))
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;

        response.recv_timeout(Duration::from_secs(5)).map_err(|_| {
            FlowError::Audio(
                "The microphone is not responding. Restart Flow to reconnect it.".into(),
            )
        })?
    }

    pub fn stop(&self, session_id: u64) -> Result<CapturedAudio> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Stop { session_id, reply })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;

        response.recv_timeout(Duration::from_secs(5)).map_err(|_| {
            FlowError::Audio(
                "The microphone is not responding. Restart Flow to reconnect it.".into(),
            )
        })?
    }

    pub fn cancel(&self, session_id: u64) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Cancel { session_id, reply })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;

        response.recv_timeout(Duration::from_secs(5)).map_err(|_| {
            FlowError::Audio(
                "The microphone is not responding. Restart Flow to reconnect it.".into(),
            )
        })?
    }

    pub fn start_mic_test(
        &self,
        session_id: u64,
        app: AppHandle,
        microphone_id: &str,
    ) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::StartMicTest {
                session_id,
                app,
                microphone_id: microphone_id.into(),
                reply,
            })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;

        response.recv_timeout(Duration::from_secs(5)).map_err(|_| {
            FlowError::Audio(
                "The microphone is not responding. Restart Flow to reconnect it.".into(),
            )
        })?
    }

    pub fn stop_mic_test(&self, session_id: u64) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::StopMicTest { session_id, reply })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;

        response.recv_timeout(Duration::from_secs(5)).map_err(|_| {
            FlowError::Audio(
                "The microphone is not responding. Restart Flow to reconnect it.".into(),
            )
        })?
    }
}

fn recorder_worker(
    receiver: mpsc::Receiver<RecorderCommand>,
    recording_flag: Arc<AtomicBool>,
    error_sender: mpsc::Sender<RecorderCommand>,
) {
    let mut active: Option<ActiveRecording> = None;
    let mut active_test: Option<ActiveRecording> = None;

    while let Ok(command) = receiver.recv() {
        match command {
            RecorderCommand::Start(params) => {
                let StartParams {
                    session_id,
                    app,
                    microphone_id,
                    target,
                    spool,
                    reply,
                } = *params;
                if active.is_some() || active_test.is_some() {
                    let _ = reply.send(Err(FlowError::AlreadyRecording));
                    continue;
                }
                match begin_recording(
                    session_id,
                    app,
                    &microphone_id,
                    target,
                    spool,
                    error_sender.clone(),
                ) {
                    Ok(recording) => {
                        active = Some(recording);
                        recording_flag.store(true, Ordering::Release);
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        recording_flag.store(false, Ordering::Release);
                        let _ = reply.send(Err(error));
                    }
                }
            }
            RecorderCommand::Stop { session_id, reply } => {
                if let Some(mut rec) = active.take() {
                    if rec.session_id != session_id {
                        active = Some(rec);
                        let _ = reply.send(Err(FlowError::NotRecording));
                        continue;
                    }
                    recording_flag.store(false, Ordering::Release);
                    let result = finish_recording(&mut rec);
                    let _ = reply.send(result);
                } else {
                    recording_flag.store(false, Ordering::Release);
                    let _ = reply.send(Err(FlowError::NotRecording));
                }
            }
            RecorderCommand::Cancel { session_id, reply } => {
                if let Some(mut rec) = active.take() {
                    if rec.session_id != session_id {
                        active = Some(rec);
                        let _ = reply.send(Ok(()));
                        continue;
                    }
                    recording_flag.store(false, Ordering::Release);
                    rec.stream.take();
                    rec.stop_capture.store(true, Ordering::Release);
                    if let Some(thread) = rec.capture_thread.take() {
                        let _ = thread.join();
                    }
                    let _ = reply.send(Ok(()));
                } else {
                    recording_flag.store(false, Ordering::Release);
                    let _ = reply.send(Ok(()));
                }
            }
            RecorderCommand::StartMicTest {
                session_id,
                app,
                microphone_id,
                reply,
            } => {
                if active.is_some() || active_test.is_some() {
                    let _ = reply.send(Err(FlowError::AlreadyRecording));
                    continue;
                }
                let dummy_target = TargetWindow {
                    hwnd: 0,
                    cursor_x: 0,
                    cursor_y: 0,
                    pid: 0,
                    session_id,
                };
                match begin_recording(
                    session_id,
                    app,
                    &microphone_id,
                    dummy_target,
                    None,
                    error_sender.clone(),
                ) {
                    Ok(recording) => {
                        active_test = Some(recording);
                        let _ = reply.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            RecorderCommand::StopMicTest { session_id, reply } => {
                if let Some(mut test) = active_test.take() {
                    if test.session_id == session_id {
                        test.stream.take();
                        test.stop_capture.store(true, Ordering::Release);
                        if let Some(thread) = test.capture_thread.take() {
                            let _ = thread.join();
                        }
                        let _ = reply.send(Ok(()));
                    } else {
                        active_test = Some(test);
                        let _ = reply
                            .send(Err(FlowError::Audio("Mismatched mic test session.".into())));
                    }
                } else {
                    let _ = reply.send(Ok(()));
                }
            }
            RecorderCommand::CaptureLimitReached { session_id, app } => {
                let Some(mut rec) = active.take() else {
                    continue;
                };
                if rec.session_id != session_id {
                    active = Some(rec);
                    continue;
                }
                crate::platform::set_recording(false);
                recording_flag.store(false, Ordering::Release);
                let result = finish_recording(&mut rec);
                match result {
                    Ok(captured) => crate::workflow::process_captured_in_background(&app, captured),
                    Err(error) => crate::workflow::report_audio_fault(&app, error),
                }
            }
            RecorderCommand::StreamFailed {
                session_id,
                app,
                message,
            } => {
                if let Some(mut test) = active_test.take() {
                    if test.session_id == session_id {
                        test.stream.take();
                        test.stop_capture.store(true, Ordering::Release);
                        if let Some(thread) = test.capture_thread.take() {
                            let _ = thread.join();
                        }
                        crate::workflow::report_audio_fault(&app, FlowError::Audio(message));
                        continue;
                    }
                    active_test = Some(test);
                }
                let Some(mut rec) = active.take() else {
                    continue;
                };
                if rec.session_id != session_id {
                    active = Some(rec);
                    continue;
                }
                crate::platform::set_recording(false);
                recording_flag.store(false, Ordering::Release);

                // Attempt to rescue partial prefix
                let rescue = finish_recording(&mut rec);
                match rescue {
                    Ok(mut captured) => {
                        captured.partial = true;
                        crate::workflow::process_captured_in_background(&app, captured);
                    }
                    Err(_) => {
                        crate::workflow::report_audio_fault(&app, FlowError::Audio(message));
                    }
                }
            }
        }
    }
}

fn begin_recording(
    session_id: u64,
    app: AppHandle,
    microphone_id: &str,
    target: TargetWindow,
    spool: Option<RecoverySpool>,
    error_sender: mpsc::Sender<RecorderCommand>,
) -> Result<ActiveRecording> {
    let host = cpal::default_host();
    let device = select_device(&host, microphone_id)?;
    let supported = device.default_input_config().map_err(|error| {
        FlowError::Audio(format!("Could not use the selected microphone: {error}"))
    })?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = config.channels as usize;
    let ring = HeapRb::<f32>::new(config.sample_rate.max(1) as usize * channels.max(1) * 2);
    let (producer, consumer) = ring.split();
    let stop_capture = Arc::new(AtomicBool::new(false));
    let overflowed = Arc::new(AtomicBool::new(false));
    let worker_app = app.clone();
    let worker_error_sender = error_sender.clone();

    let stream_result = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            session_id,
            |sample| sample,
            error_sender,
        ),
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            session_id,
            |sample| sample as f32 / i16::MAX as f32,
            error_sender,
        ),
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            session_id,
            |sample| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0,
            error_sender,
        ),
        format => Err(FlowError::Audio(format!(
            "Unsupported microphone sample format: {format:?}"
        ))),
    };

    let stream = stream_result?;
    let capture_thread = spawn_capture_worker(
        consumer,
        config.sample_rate,
        channels,
        stop_capture.clone(),
        overflowed,
        worker_app,
        session_id,
        spool,
        worker_error_sender,
    )?;

    let recording = ActiveRecording {
        session_id,
        stream: Some(stream),
        capture_thread: Some(capture_thread),
        stop_capture,
        target,
    };

    recording
        .stream
        .as_ref()
        .expect("the recording stream was just created")
        .play()
        .map_err(|error| FlowError::Audio(format!("Could not start the microphone: {error}")))?;

    Ok(recording)
}

fn finish_recording(recording: &mut ActiveRecording) -> Result<CapturedAudio> {
    recording.stream.take();
    recording.stop_capture.store(true, Ordering::Release);
    let captured = recording
        .capture_thread
        .take()
        .ok_or_else(|| FlowError::Audio("Recorded audio is unavailable.".into()))?
        .join()
        .map_err(|_| FlowError::Audio("The audio capture worker stopped unexpectedly.".into()))?;

    let duration_ms = (captured.sample_count as i64 * 1000) / 16_000;

    if captured.sample_count < 1_600 {
        return Err(FlowError::EmptyRecording);
    }
    if !captured.audible && !captured.partial {
        return Err(FlowError::EmptyRecording);
    }

    Ok(CapturedAudio {
        wav: captured.wav,
        duration_ms,
        target: recording.target,
        partial: captured.partial,
        limit_reached: captured.limit_reached,
    })
}

fn select_device(host: &cpal::Host, microphone_id: &str) -> Result<Device> {
    if microphone_id.is_empty() || microphone_id == "default" {
        return host
            .default_input_device()
            .ok_or_else(|| FlowError::Audio("No microphone was found.".into()));
    }

    let device_id = microphone_id
        .parse::<DeviceId>()
        .unwrap_or_else(|_| DeviceId(cpal::platform::HostId::Wasapi, microphone_id.into()));

    if let Some(device) = host.device_by_id(&device_id) {
        if device.supports_input() {
            return Ok(device);
        }
    }

    // F19: Explicit modern device IDs must fail strictly and NOT migrate to another device
    let is_explicit_id = microphone_id.starts_with('{')
        || microphone_id.contains("Wasapi")
        || microphone_id.contains('\u{1f}');

    if is_explicit_id {
        return Err(FlowError::Audio(
            "The selected microphone is unavailable.".into(),
        ));
    }

    // Allow legacy name migration only if exactly one endpoint matches uniquely
    let devices = host
        .input_devices()
        .map_err(|error| FlowError::Audio(format!("Could not enumerate microphones: {error}")))?;

    let mut matching_devices = Vec::new();
    for device in devices {
        if let Ok(description) = device.description() {
            if description.name() == microphone_id {
                matching_devices.push(device);
            }
        }
    }

    if matching_devices.len() == 1 {
        return Ok(matching_devices.remove(0));
    }

    Err(FlowError::Audio(
        "The selected microphone is unavailable.".into(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut producer: HeapProd<f32>,
    overflowed: Arc<AtomicBool>,
    app: AppHandle,
    session_id: u64,
    convert: fn(T) -> f32,
    error_sender: mpsc::Sender<RecorderCommand>,
) -> Result<Stream>
where
    T: cpal::SizedSample + Copy + Send + 'static,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for sample in data {
                    let s = convert(*sample);
                    if producer.try_push(s).is_err() {
                        overflowed.store(true, Ordering::Release);
                    }
                }
            },
            move |error| {
                let _ = error_sender.send(RecorderCommand::StreamFailed {
                    session_id,
                    app: app.clone(),
                    message: format!("The microphone stream stopped: {error}"),
                });
            },
            Some(Duration::from_millis(80)),
        )
        .map_err(|error| FlowError::Audio(format!("Could not open the microphone: {error}")))
}

#[allow(clippy::too_many_arguments)]
fn spawn_capture_worker(
    mut consumer: HeapCons<f32>,
    input_rate: u32,
    channels: usize,
    stop: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    app: AppHandle,
    session_id: u64,
    mut spool: Option<RecoverySpool>,
    error_sender: mpsc::Sender<RecorderCommand>,
) -> Result<std::thread::JoinHandle<EncodedCapture>> {
    if !(8_000..=192_000).contains(&input_rate) || !(1..=32).contains(&channels) {
        return Err(FlowError::Audio(format!(
            "Invalid audio stream configuration: {input_rate} Hz, {channels} channels."
        )));
    }

    let chunk_size = (input_rate as usize * 20) / 1000; // 20ms chunk
    let resampler = if input_rate as usize != 16_000 {
        Some(
            FftFixedIn::<f32>::new(input_rate as usize, 16_000, chunk_size, 1, 1)
                .map_err(|e| FlowError::Audio(format!("Could not create audio resampler: {e}")))?,
        )
    } else {
        None
    };

    std::thread::Builder::new()
        .name("flow-audio-capture".into())
        .spawn(move || {
            const TARGET_RATE: usize = 16_000;
            const MAX_OUTPUT_SAMPLES: usize = 16_000 * 5 * 60; // 5 mins
            let mut wav = vec![0_u8; 44];
            wav.reserve(16_000 * 2 * 30);

            // Channel selection: buffer initial 100ms
            let initial_frames_cap = (input_rate as usize * 100) / 1000;
            let mut initial_buffer: Vec<f32> = Vec::new();
            let mut selected_channel: Option<usize> = if channels <= 1 { Some(0) } else { None };

            let mut resampler = resampler;
            let delay_to_skip = resampler.as_ref().map(|r| r.output_delay()).unwrap_or(0);
            let mut delay_skipped = 0;

            let mut output_samples = 0_usize;
            let mut total_input_frames = 0_usize;
            let mut input_accumulator: Vec<f32> = Vec::with_capacity(chunk_size * 2);
            let mut interleaved_frame_buf: Vec<f32> = Vec::with_capacity(channels * 4);
            let mut peak = 0.0_f32;
            let mut last_emit = Instant::now();
            let mut limit_reported = false;
            let mut overflow_reported = false;
            let mut partial_corrupted = false;
            let mut audibility = AudibilityDetector::new();

            loop {
                let mut consumed = false;

                while let Some(sample) = consumer.try_pop() {
                    consumed = true;
                    let s = if sample.is_nan() || sample.is_infinite() {
                        partial_corrupted = true;
                        0.0_f32
                    } else {
                        sample
                    };
                    interleaved_frame_buf.push(s);
                }

                let full_frames_count = interleaved_frame_buf.len() / channels;
                if full_frames_count > 0 {
                    let full_samples_count = full_frames_count * channels;
                    let available_samples: Vec<f32> = interleaved_frame_buf.drain(..full_samples_count).collect();

                    match selected_channel {
                        None => {
                            initial_buffer.extend_from_slice(&available_samples);
                            if initial_buffer.len() >= initial_frames_cap * channels {
                                let mut channel_sums = vec![0.0f32; channels];
                                let num_frames = initial_buffer.len() / channels;
                                for frame in initial_buffer.chunks_exact(channels) {
                                    for (ch, &s) in frame.iter().enumerate() {
                                        channel_sums[ch] += s * s;
                                    }
                                }
                                let mut best_ch = 0;
                                let mut max_rms = -1.0f32;
                                for (ch, &sum) in channel_sums.iter().enumerate() {
                                    let rms = (sum / num_frames as f32).sqrt();
                                    if rms > max_rms {
                                        max_rms = rms;
                                        best_ch = ch;
                                    }
                                }
                                let is_nontrivial = max_rms > 0.0001;
                                let max_initial_wait = input_rate as usize * channels;
                                if is_nontrivial || initial_buffer.len() >= max_initial_wait {
                                    selected_channel = Some(best_ch);
                                    for frame in initial_buffer.chunks_exact(channels) {
                                        let s = frame[best_ch];
                                        peak = peak.max(s.abs());
                                        input_accumulator.push(s);
                                        total_input_frames += 1;
                                    }
                                    initial_buffer.clear();
                                }
                            }
                        }
                        Some(ch) => {
                            for frame in available_samples.chunks_exact(channels) {
                                let s = frame[ch];
                                peak = peak.max(s.abs());
                                input_accumulator.push(s);
                                total_input_frames += 1;
                            }
                        }
                    }
                }

                // Resample available input chunks
                if let Some(res) = resampler.as_mut() {
                    while input_accumulator.len() >= chunk_size {
                        let chunk: Vec<f32> = input_accumulator.drain(..chunk_size).collect();
                        if let Ok(out) = res.process(&[&chunk], None) {
                            if let Some(out_frames) = out.first() {
                                let mut pcm_batch = Vec::with_capacity(out_frames.len() * 2);
                                for &s in out_frames {
                                    if delay_skipped < delay_to_skip {
                                        delay_skipped += 1;
                                        continue;
                                    }
                                    if output_samples < MAX_OUTPUT_SAMPLES {
                                        let val = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                        let bytes = val.to_le_bytes();
                                        wav.extend_from_slice(&bytes);
                                        pcm_batch.extend_from_slice(&bytes);
                                        output_samples += 1;
                                        audibility.push(s);
                                    } else if !limit_reported {
                                        limit_reported = true;
                                        stop.store(true, Ordering::Release);
                                        crate::platform::set_recording(false);
                                        let _ = app.emit(
                                            "flow-warning",
                                            MessagePayload {
                                                message: "The recording reached the five-minute limit and will be saved for review.".into(),
                                            },
                                        );
                                    }
                                }
                                if !pcm_batch.is_empty() {
                                    if let Some(s_spool) = spool.as_mut() {
                                        let _ = s_spool.write_pcm(&pcm_batch);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Direct 16kHz
                    let mut pcm_batch = Vec::new();
                    while !input_accumulator.is_empty() {
                        let s = input_accumulator.remove(0);
                        if output_samples < MAX_OUTPUT_SAMPLES {
                            let val = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            let bytes = val.to_le_bytes();
                            wav.extend_from_slice(&bytes);
                            pcm_batch.extend_from_slice(&bytes);
                            output_samples += 1;
                            audibility.push(s);
                        } else if !limit_reported {
                            limit_reported = true;
                            stop.store(true, Ordering::Release);
                            crate::platform::set_recording(false);
                            let _ = app.emit(
                                "flow-warning",
                                MessagePayload {
                                    message: "The recording reached the five-minute limit and will be saved for review.".into(),
                                },
                            );
                        }
                    }
                    if !pcm_batch.is_empty() {
                        if let Some(s_spool) = spool.as_mut() {
                            let _ = s_spool.write_pcm(&pcm_batch);
                        }
                    }
                }

                if last_emit.elapsed() >= Duration::from_millis(33) {
                    let responsive = (peak * 3.5).sqrt().min(1.0);
                    let _ = app.emit_to("overlay", "waveform", WaveformPayload { level: responsive });
                    let _ = app.emit("audio-level", WaveformPayload { level: responsive });
                    peak = 0.0;
                    last_emit = Instant::now();
                }

                if overflowed.swap(false, Ordering::AcqRel) && !overflow_reported {
                    overflow_reported = true;
                    let _ = error_sender.send(RecorderCommand::StreamFailed {
                        session_id,
                        app: app.clone(),
                        message: "Audio capture overflowed ring buffer.".into(),
                    });
                }

                if stop.load(Ordering::Acquire) && !consumed {
                    break;
                }
                if !consumed {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }

            // Flush remaining partial input and filter tail
            let target_output_samples = ((total_input_frames as f64 * TARGET_RATE as f64) / input_rate as f64).round() as usize;
            let target_output_samples = target_output_samples.min(MAX_OUTPUT_SAMPLES);

            if let Some(res) = resampler.as_mut() {
                while output_samples < target_output_samples {
                    let mut chunk = Vec::with_capacity(chunk_size);
                    if !input_accumulator.is_empty() {
                        let take_n = input_accumulator.len().min(chunk_size);
                        chunk.extend(input_accumulator.drain(..take_n));
                    }
                    if chunk.len() < chunk_size {
                        chunk.resize(chunk_size, 0.0_f32);
                    }
                    if let Ok(out) = res.process(&[&chunk], None) {
                        if let Some(out_frames) = out.first() {
                            let mut pcm_batch = Vec::new();
                            for &s in out_frames {
                                if delay_skipped < delay_to_skip {
                                    delay_skipped += 1;
                                    continue;
                                }
                                if output_samples < target_output_samples && output_samples < MAX_OUTPUT_SAMPLES {
                                    let val = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                    let bytes = val.to_le_bytes();
                                    wav.extend_from_slice(&bytes);
                                    pcm_batch.extend_from_slice(&bytes);
                                    output_samples += 1;
                                    audibility.push(s);
                                }
                            }
                            if !pcm_batch.is_empty() {
                                if let Some(s_spool) = spool.as_mut() {
                                    let _ = s_spool.write_pcm(&pcm_batch);
                                }
                            }
                        }
                    } else {
                        break;
                    }
                }
            } else {
                let mut pcm_batch = Vec::new();
                while !input_accumulator.is_empty() && output_samples < target_output_samples && output_samples < MAX_OUTPUT_SAMPLES {
                    let s = input_accumulator.remove(0);
                    let val = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    let bytes = val.to_le_bytes();
                    wav.extend_from_slice(&bytes);
                    pcm_batch.extend_from_slice(&bytes);
                    output_samples += 1;
                    audibility.push(s);
                }
                if !pcm_batch.is_empty() {
                    if let Some(s_spool) = spool.as_mut() {
                        let _ = s_spool.write_pcm(&pcm_batch);
                    }
                }
            }

            if let Some(s_spool) = spool.as_mut() {
                let _ = s_spool.flush_and_sync();
            }

            write_wav_header(&mut wav[..44], output_samples, 16_000);
            let captured = EncodedCapture {
                wav,
                sample_count: output_samples,
                audible: audibility.audible,
                partial: overflow_reported || partial_corrupted,
                limit_reached: limit_reported,
            };

            if limit_reported {
                let _ = error_sender.send(RecorderCommand::CaptureLimitReached { session_id, app });
            }

            captured
        })
        .map_err(|error| {
            FlowError::Audio(format!("Could not start the audio capture worker: {error}"))
        })
}

fn write_wav_header(header: &mut [u8], sample_count: usize, sample_rate: u32) {
    let data_bytes = (sample_count * 2) as u32;
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&1_u16.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(sample_rate * 2).to_le_bytes());
    header[32..34].copy_from_slice(&2_u16.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
}

pub fn list_microphones() -> Result<Vec<Microphone>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|error| FlowError::Audio(format!("Could not enumerate microphones: {error}")))?;
    let default_id = host
        .default_input_device()
        .and_then(|device| device.id().ok());
    let mut result = Vec::new();
    for device in devices {
        let name = match device.description() {
            Ok(desc) => desc.name().to_owned(),
            Err(_) => continue, // Skip unreadable endpoint per F19
        };
        let id = match device.id() {
            Ok(id) => id.to_string(),
            Err(_) => continue,
        };
        result.push(Microphone {
            is_default: default_id
                .as_ref()
                .map(|d| d.to_string() == id)
                .unwrap_or(false),
            id,
            name,
            is_available: true,
        });
    }
    result.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(result)
}

struct AudibilityDetector {
    audible: bool,
    samples: usize,
    peak: f32,
    voiced_windows: usize,
}

impl AudibilityDetector {
    fn new() -> Self {
        Self {
            audible: false,
            samples: 0,
            peak: 0.0,
            voiced_windows: 0,
        }
    }

    fn push(&mut self, sample: f32) {
        self.samples += 1;
        self.peak = self.peak.max(sample.abs());
        if self.samples >= 800 {
            if self.peak >= 0.035 {
                self.voiced_windows += 1;
                if self.voiced_windows >= 3 {
                    self.audible = true;
                }
            }
            self.peak = 0.0;
            self.samples = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulate_resample(input: &[f32], input_rate: u32) -> Vec<f32> {
        let chunk_size = (input_rate as usize * 20) / 1000;
        let mut resampler = if input_rate as usize != 16_000 {
            Some(FftFixedIn::<f32>::new(input_rate as usize, 16_000, chunk_size, 1, 1).unwrap())
        } else {
            None
        };

        let delay_to_skip = resampler.as_ref().map(|r| r.output_delay()).unwrap_or(0);
        let mut delay_skipped = 0;
        let mut output = Vec::new();
        let mut accumulator = Vec::new();

        let target_output_samples =
            ((input.len() as f64 * 16_000.0) / input_rate as f64).round() as usize;

        if let Some(res) = resampler.as_mut() {
            for &s in input {
                accumulator.push(s);
                if accumulator.len() >= chunk_size {
                    let chunk: Vec<f32> = accumulator.drain(..chunk_size).collect();
                    let out = res.process(&[&chunk], None).unwrap();
                    for &sample in out.first().unwrap() {
                        if delay_skipped < delay_to_skip {
                            delay_skipped += 1;
                            continue;
                        }
                        if output.len() < target_output_samples {
                            output.push(sample);
                        }
                    }
                }
            }

            // Flush tail
            while output.len() < target_output_samples {
                let mut chunk = Vec::with_capacity(chunk_size);
                if !accumulator.is_empty() {
                    let take_n = accumulator.len().min(chunk_size);
                    chunk.extend(accumulator.drain(..take_n));
                }
                if chunk.len() < chunk_size {
                    chunk.resize(chunk_size, 0.0_f32);
                }
                let out = res.process(&[&chunk], None).unwrap();
                for &sample in out.first().unwrap() {
                    if delay_skipped < delay_to_skip {
                        delay_skipped += 1;
                        continue;
                    }
                    if output.len() < target_output_samples {
                        output.push(sample);
                    }
                }
            }
        } else {
            output.extend_from_slice(input);
        }

        output
    }

    #[test]
    fn test_probe_28_resampling_one_second_produces_exact_16000_samples() {
        for &rate in &[44_100, 48_000, 96_000] {
            let input = vec![0.1_f32; rate as usize];
            let out = simulate_resample(&input, rate);
            assert_eq!(
                out.len(),
                16_000,
                "Failed for input rate {rate}: expected 16000 samples, got {}",
                out.len()
            );
        }
    }

    #[test]
    fn test_resample_16k_passthrough() {
        let input = vec![0.25_f32; 16_000];
        let out = simulate_resample(&input, 16_000);
        assert_eq!(out.len(), 16_000);
    }
}
