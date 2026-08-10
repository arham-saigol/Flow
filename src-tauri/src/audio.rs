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
use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};
use tauri::{AppHandle, Emitter};

use crate::{
    error::{FlowError, Result},
    models::{MessagePayload, Microphone, WaveformPayload},
    platform::TargetWindow,
};

pub struct CapturedAudio {
    pub wav: Vec<u8>,
    pub duration_ms: i64,
    pub target: TargetWindow,
}

struct ActiveRecording {
    stream: Option<Stream>,
    capture_thread: Option<std::thread::JoinHandle<EncodedCapture>>,
    stop_capture: Arc<AtomicBool>,
    started: Instant,
    target: TargetWindow,
}

struct EncodedCapture {
    wav: Vec<u8>,
    sample_count: usize,
    audible: bool,
}

struct AudibilityDetector {
    sum: f32,
    samples: usize,
    audible_windows: usize,
    audible: bool,
}

impl AudibilityDetector {
    fn new() -> Self {
        Self {
            sum: 0.0,
            samples: 0,
            audible_windows: 0,
            audible: false,
        }
    }

    fn push(&mut self, sample: f32) {
        self.sum += sample * sample;
        self.samples += 1;
        if self.samples == 320 {
            if (self.sum / self.samples as f32).sqrt() >= 0.003 {
                self.audible_windows += 1;
                self.audible |= self.audible_windows >= 3;
            } else {
                self.audible_windows = 0;
            }
            self.sum = 0.0;
            self.samples = 0;
        }
    }
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
}

enum RecorderCommand {
    Start {
        app: AppHandle,
        microphone_id: String,
        target: TargetWindow,
        reply: mpsc::SyncSender<Result<()>>,
    },
    Stop {
        reply: mpsc::SyncSender<Result<CapturedAudio>>,
    },
    Cancel {
        reply: mpsc::SyncSender<Result<()>>,
    },
    CaptureLimitReached {
        app: AppHandle,
    },
    StreamFailed {
        app: AppHandle,
        message: String,
    },
}

impl AudioRecorder {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let worker_recording = Arc::new(AtomicBool::new(false));
        let error_sender = sender.clone();
        std::thread::Builder::new()
            .name("flow-audio".into())
            .spawn(move || recorder_worker(receiver, worker_recording, error_sender))
            .expect("could not start Flow audio worker");
        Self { sender }
    }

    pub fn start(&self, app: AppHandle, microphone_id: &str, target: TargetWindow) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Start {
                app,
                microphone_id: microphone_id.into(),
                target,
                reply,
            })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;
        response
            .recv()
            .map_err(|_| FlowError::Audio("The audio worker did not respond.".into()))?
    }

    pub fn stop(&self) -> Result<CapturedAudio> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Stop { reply })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;
        response
            .recv()
            .map_err(|_| FlowError::Audio("The audio worker did not respond.".into()))?
    }

    pub fn cancel(&self) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .send(RecorderCommand::Cancel { reply })
            .map_err(|_| FlowError::Audio("The audio worker stopped unexpectedly.".into()))?;
        response
            .recv()
            .map_err(|_| FlowError::Audio("The audio worker did not respond.".into()))?
    }
}

fn recorder_worker(
    receiver: mpsc::Receiver<RecorderCommand>,
    recording_flag: Arc<AtomicBool>,
    error_sender: mpsc::Sender<RecorderCommand>,
) {
    let mut active: Option<ActiveRecording> = None;
    while let Ok(command) = receiver.recv() {
        match command {
            RecorderCommand::Start {
                app,
                microphone_id,
                target,
                reply,
            } => {
                let result = if active.is_some() {
                    Err(FlowError::AlreadyRecording)
                } else {
                    begin_recording(app, &microphone_id, target, error_sender.clone()).map(
                        |recording| {
                            active = Some(recording);
                            recording_flag.store(true, Ordering::Release);
                        },
                    )
                };
                let _ = reply.send(result);
            }
            RecorderCommand::Stop { reply } => {
                let result = active
                    .take()
                    .ok_or(FlowError::NotRecording)
                    .and_then(finish_recording);
                recording_flag.store(false, Ordering::Release);
                let _ = reply.send(result);
            }
            RecorderCommand::Cancel { reply } => {
                let result = if active.take().is_some() {
                    Ok(())
                } else {
                    Err(FlowError::NotRecording)
                };
                recording_flag.store(false, Ordering::Release);
                let _ = reply.send(result);
            }
            RecorderCommand::CaptureLimitReached { app } => {
                let Some(recording) = active.take() else {
                    continue;
                };
                let result = finish_recording(recording);
                crate::platform::set_recording(false);
                recording_flag.store(false, Ordering::Release);
                match result {
                    Ok(captured) => crate::workflow::process_captured_in_background(&app, captured),
                    Err(error) => crate::workflow::report_error(&app, error),
                }
            }
            RecorderCommand::StreamFailed { app, message } => {
                if active.take().is_some() {
                    recording_flag.store(false, Ordering::Release);
                    crate::workflow::report_error(&app, FlowError::Audio(message));
                }
            }
        }
    }
}

fn begin_recording(
    app: AppHandle,
    microphone_id: &str,
    target: TargetWindow,
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
    let ring = HeapRb::<f32>::new(config.sample_rate as usize * 2);
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
            channels,
            |sample| sample,
            error_sender,
        ),
        SampleFormat::F64 => build_stream::<f64>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| sample as f32,
            error_sender,
        ),
        SampleFormat::I8 => build_stream::<i8>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| sample as f32 / i8::MAX as f32,
            error_sender,
        ),
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| sample as f32 / i16::MAX as f32,
            error_sender,
        ),
        SampleFormat::I32 => build_stream::<i32>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| sample as f32 / i32::MAX as f32,
            error_sender,
        ),
        SampleFormat::I64 => build_stream::<i64>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| (sample as f64 / i64::MAX as f64) as f32,
            error_sender,
        ),
        SampleFormat::U8 => build_stream::<u8>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            u8_to_f32,
            error_sender,
        ),
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0,
            error_sender,
        ),
        SampleFormat::U32 => build_stream::<u32>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| (sample as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32,
            error_sender,
        ),
        SampleFormat::U64 => build_stream::<u64>(
            &device,
            &config,
            producer,
            overflowed.clone(),
            app,
            channels,
            |sample| (sample as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32,
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
        stop_capture.clone(),
        overflowed,
        worker_app,
        worker_error_sender,
    )?;
    let recording = ActiveRecording {
        stream: Some(stream),
        capture_thread: Some(capture_thread),
        stop_capture,
        started: Instant::now(),
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

fn finish_recording(mut recording: ActiveRecording) -> Result<CapturedAudio> {
    let duration_ms = recording.started.elapsed().as_millis() as i64;
    recording.stream.take();
    recording.stop_capture.store(true, Ordering::Release);
    let captured = recording
        .capture_thread
        .take()
        .ok_or_else(|| FlowError::Audio("Recorded audio is unavailable.".into()))?
        .join()
        .map_err(|_| FlowError::Audio("The audio capture worker stopped unexpectedly.".into()))?;
    if captured.sample_count < 2_000 {
        return Err(FlowError::EmptyRecording);
    }
    if !captured.audible {
        return Err(FlowError::EmptyRecording);
    }
    Ok(CapturedAudio {
        wav: captured.wav,
        duration_ms,
        target: recording.target,
    })
}

fn u8_to_f32(sample: u8) -> f32 {
    (sample as f32 - 128.0) / 128.0
}

fn select_device(host: &cpal::Host, microphone_id: &str) -> Result<Device> {
    if microphone_id.is_empty() {
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
    let devices = host
        .input_devices()
        .map_err(|error| FlowError::Audio(format!("Could not enumerate microphones: {error}")))?;
    let fallback_name = microphone_id
        .split_once('\u{1f}')
        .map_or(microphone_id, |(_, name)| name);
    for device in devices {
        if device
            .description()
            .map(|description| description.name() == fallback_name)
            .unwrap_or(false)
        {
            return Ok(device);
        }
    }
    host.default_input_device().ok_or_else(|| {
        FlowError::Audio("The selected microphone is unavailable and no default was found.".into())
    })
}

#[allow(clippy::too_many_arguments)] // Keeps the format-specific callback setup explicit.
fn build_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut producer: HeapProd<f32>,
    overflowed: Arc<AtomicBool>,
    app: AppHandle,
    channels: usize,
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
                for frame in data.chunks(channels.max(1)) {
                    let sample =
                        frame.iter().copied().map(convert).sum::<f32>() / frame.len() as f32;
                    if producer.try_push(sample).is_err() {
                        overflowed.store(true, Ordering::Release);
                    }
                }
            },
            move |error| {
                let _ = error_sender.send(RecorderCommand::StreamFailed {
                    app: app.clone(),
                    message: format!("The microphone stream stopped: {error}"),
                });
            },
            Some(Duration::from_millis(80)),
        )
        .map_err(|error| FlowError::Audio(format!("Could not open the microphone: {error}")))
}

fn spawn_capture_worker(
    mut consumer: HeapCons<f32>,
    input_rate: u32,
    stop: Arc<AtomicBool>,
    overflowed: Arc<AtomicBool>,
    app: AppHandle,
    error_sender: mpsc::Sender<RecorderCommand>,
) -> Result<std::thread::JoinHandle<EncodedCapture>> {
    std::thread::Builder::new()
        .name("flow-audio-capture".into())
        .spawn(move || {
            const OUTPUT_RATE: u64 = 16_000;
            const MAX_OUTPUT_SAMPLES: usize = 16_000 * 5 * 60;
            let mut wav = vec![0_u8; 44];
            wav.reserve(16_000 * 2 * 30);
            let mut output_samples = 0_usize;
            let mut accumulator = 0_u64;
            let mut window_sum = 0.0_f32;
            let mut window_len = 0_u32;
            let mut peak = 0.0_f32;
            let mut last_emit = Instant::now();
            let mut limit_reported = false;
            let mut overflow_reported = false;
            let mut audibility = AudibilityDetector::new();

            loop {
                let mut consumed = false;
                while let Some(sample) = consumer.try_pop() {
                    consumed = true;
                    peak = peak.max(sample.abs());
                    window_sum += sample;
                    window_len += 1;
                    accumulator += OUTPUT_RATE;
                    while accumulator >= u64::from(input_rate) {
                        accumulator -= u64::from(input_rate);
                        let averaged = if window_len == 0 {
                            sample
                        } else {
                            window_sum / window_len as f32
                        };
                        window_sum = 0.0;
                        window_len = 0;
                        if output_samples < MAX_OUTPUT_SAMPLES {
                            let value = (averaged.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                            wav.extend_from_slice(&value.to_le_bytes());
                            output_samples += 1;
                            audibility.push(averaged);
                        } else if !limit_reported {
                            limit_reported = true;
                            stop.store(true, Ordering::Release);
                            crate::platform::set_recording(false);
                            let _ = app.emit(
                                "flow-warning",
                                MessagePayload {
                                    message: "The recording reached the five-minute limit and will be processed now.".into(),
                                },
                            );
                        }
                    }
                }

                if last_emit.elapsed() >= Duration::from_millis(33) {
                    let responsive = (peak * 3.5).sqrt().min(1.0);
                    let _ =
                        app.emit_to("overlay", "waveform", WaveformPayload { level: responsive });
                    peak = 0.0;
                    last_emit = Instant::now();
                }
                if overflowed.swap(false, Ordering::AcqRel) && !overflow_reported {
                    overflow_reported = true;
                    let _ = error_sender.send(RecorderCommand::StreamFailed {
                        app: app.clone(),
                        message: "Audio capture could not keep up with the microphone.".into(),
                    });
                }
                if stop.load(Ordering::Acquire) && !consumed {
                    break;
                }
                if !consumed {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            write_wav_header(&mut wav[..44], output_samples, 16_000);
            let captured = EncodedCapture {
                wav,
                sample_count: output_samples,
                audible: audibility.audible,
            };
            if limit_reported {
                let _ = error_sender.send(RecorderCommand::CaptureLimitReached { app });
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
        let name = device
            .description()
            .map_err(|error| {
                FlowError::Audio(format!("Could not read a microphone name: {error}"))
            })?
            .name()
            .to_owned();
        let id = device.id().map_err(|error| {
            FlowError::Audio(format!(
                "Could not identify a Windows audio endpoint: {error}"
            ))
        })?;
        result.push(Microphone {
            is_default: default_id.as_ref() == Some(&id),
            id: id.to_string(),
            name,
        });
    }
    result.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{u8_to_f32, write_wav_header, AudibilityDetector};

    fn is_audible(samples: &[f32]) -> bool {
        let mut detector = AudibilityDetector::new();
        for sample in samples {
            detector.push(*sample);
        }
        detector.audible
    }

    #[test]
    fn unsigned_8_bit_silence_is_centered() {
        assert_eq!(u8_to_f32(128), 0.0);
    }

    #[test]
    fn silence_is_not_audible() {
        assert!(!is_audible(&vec![0.0; 16_000]));
    }

    #[test]
    fn a_brief_click_is_not_audible() {
        let mut samples = vec![0.0; 16_000];
        samples[1_000] = 1.0;
        assert!(!is_audible(&samples));
    }

    #[test]
    fn sustained_audio_is_audible() {
        let mut samples = vec![0.0; 16_000];
        samples[1_000..2_280].fill(0.01);
        assert!(is_audible(&samples));
    }

    #[test]
    fn wav_header_describes_incrementally_encoded_pcm() {
        let mut header = [0_u8; 44];
        write_wav_header(&mut header, 16_000, 16_000);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(header[40..44].try_into().unwrap()),
            32_000
        );
    }
}
