use std::{
    io::Cursor,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, DeviceId, SampleFormat, Stream, StreamConfig,
};
use tauri::{AppHandle, Emitter};

use crate::{
    error::{FlowError, Result},
    models::{Microphone, WaveformPayload},
    platform::TargetWindow,
};

pub struct CapturedAudio {
    pub wav: Vec<u8>,
    pub duration_ms: i64,
    pub target: TargetWindow,
}

struct ActiveRecording {
    _stream: Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    started: Instant,
    target: TargetWindow,
}

pub struct AudioRecorder {
    sender: mpsc::Sender<RecorderCommand>,
    recording: Arc<AtomicBool>,
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
    StreamFailed {
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
    let samples = Arc::new(Mutex::new(Vec::with_capacity(
        config.sample_rate as usize * 30,
    )));
    let emit_counter = Arc::new(AtomicUsize::new(0));
    let channels = config.channels as usize;
    let threshold = (config.sample_rate / 30).max(1) as usize;
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| sample,
            error_sender,
        )?,
        SampleFormat::F64 => build_stream::<f64>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| sample as f32,
            error_sender,
        )?,
        SampleFormat::I8 => build_stream::<i8>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| sample as f32 / i8::MAX as f32,
            error_sender,
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| sample as f32 / i16::MAX as f32,
            error_sender,
        )?,
        SampleFormat::I32 => build_stream::<i32>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| sample as f32 / i32::MAX as f32,
            error_sender,
        )?,
        SampleFormat::I64 => build_stream::<i64>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| (sample as f64 / i64::MAX as f64) as f32,
            error_sender,
        )?,
        SampleFormat::U8 => build_stream::<u8>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| (sample as f32 / u8::MAX as f32) * 2.0 - 1.0,
            error_sender,
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0,
            error_sender,
        )?,
        SampleFormat::U32 => build_stream::<u32>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| (sample as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32,
            error_sender,
        )?,
        SampleFormat::U64 => build_stream::<u64>(
            &device,
            &config,
            samples.clone(),
            app,
            emit_counter,
            channels,
            threshold,
            |sample| (sample as f64 / u64::MAX as f64 * 2.0 - 1.0) as f32,
            error_sender,
        )?,
        format => {
            return Err(FlowError::Audio(format!(
                "Unsupported microphone sample format: {format:?}"
            )))
        }
    };
    stream
        .play()
        .map_err(|error| FlowError::Audio(format!("Could not start the microphone: {error}")))?;
    Ok(ActiveRecording {
        _stream: stream,
        samples,
        sample_rate: config.sample_rate,
        started: Instant::now(),
        target,
    })
}

fn finish_recording(recording: ActiveRecording) -> Result<CapturedAudio> {
    let duration_ms = recording.started.elapsed().as_millis() as i64;
    drop(recording._stream);
    let samples = Arc::try_unwrap(recording.samples)
        .map_err(|_| FlowError::Audio("Could not finish the audio stream.".into()))?
        .into_inner()
        .map_err(|_| FlowError::Audio("Recorded audio is unavailable.".into()))?;
    if samples.len() < (recording.sample_rate / 8) as usize {
        return Err(FlowError::EmptyRecording);
    }
    let mono_16k = resample(&samples, recording.sample_rate, 16_000);
    let wav = encode_wav(&mono_16k, 16_000);
    Ok(CapturedAudio {
        wav,
        duration_ms,
        target: recording.target,
    })
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

fn build_stream<T>(
    device: &Device,
    config: &StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    app: AppHandle,
    counter: Arc<AtomicUsize>,
    channels: usize,
    threshold: usize,
    convert: fn(T) -> f32,
    error_sender: mpsc::Sender<RecorderCommand>,
) -> Result<Stream>
where
    T: cpal::SizedSample + Copy + Send + 'static,
{
    let error_app = app.clone();
    let limit_app = app.clone();
    let limit_sender = error_sender.clone();
    let maximum_samples = config.sample_rate as usize * 5 * 60;
    let mut limit_reported = false;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut mono = Vec::with_capacity(data.len() / channels.max(1));
                let mut peak = 0.0_f32;
                for frame in data.chunks(channels.max(1)) {
                    let sample =
                        frame.iter().copied().map(convert).sum::<f32>() / frame.len() as f32;
                    peak = peak.max(sample.abs());
                    mono.push(sample);
                }
                if let Ok(mut destination) = samples.lock() {
                    let remaining = maximum_samples.saturating_sub(destination.len());
                    let accepted = remaining.min(mono.len());
                    destination.extend_from_slice(&mono[..accepted]);
                    if accepted < mono.len() && !limit_reported {
                        limit_reported = true;
                        let _ = limit_sender.send(RecorderCommand::StreamFailed {
                            app: limit_app.clone(),
                            message: "The recording reached the five-minute limit.".into(),
                        });
                    }
                }
                if counter.fetch_add(mono.len(), Ordering::Relaxed) + mono.len() >= threshold {
                    counter.store(0, Ordering::Relaxed);
                    let responsive = (peak * 3.5).sqrt().min(1.0);
                    let _ =
                        app.emit_to("overlay", "waveform", WaveformPayload { level: responsive });
                }
            },
            move |error| {
                let _ = error_sender.send(RecorderCommand::StreamFailed {
                    app: error_app.clone(),
                    message: format!("The microphone stream stopped: {error}"),
                });
            },
            Some(Duration::from_millis(80)),
        )
        .map_err(|error| FlowError::Audio(format!("Could not open the microphone: {error}")))
}

fn resample(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if input_rate == output_rate {
        return input.to_vec();
    }
    let ratio = input_rate as f64 / output_rate as f64;
    let output_len = (input.len() as f64 / ratio) as usize;
    (0..output_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let low = position.floor() as usize;
            let high = (low + 1).min(input.len().saturating_sub(1));
            let fraction = (position - low as f64) as f32;
            input[low] * (1.0 - fraction) + input[high] * fraction
        })
        .collect()
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let mut output = Cursor::new(Vec::with_capacity(data_bytes as usize + 44));
    use std::io::Write;
    let _ = output.write_all(b"RIFF");
    let _ = output.write_all(&(36 + data_bytes).to_le_bytes());
    let _ = output.write_all(b"WAVEfmt ");
    let _ = output.write_all(&16_u32.to_le_bytes());
    let _ = output.write_all(&1_u16.to_le_bytes());
    let _ = output.write_all(&1_u16.to_le_bytes());
    let _ = output.write_all(&sample_rate.to_le_bytes());
    let _ = output.write_all(&(sample_rate * 2).to_le_bytes());
    let _ = output.write_all(&2_u16.to_le_bytes());
    let _ = output.write_all(&16_u16.to_le_bytes());
    let _ = output.write_all(b"data");
    let _ = output.write_all(&data_bytes.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        let _ = output.write_all(&value.to_le_bytes());
    }
    output.into_inner()
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
