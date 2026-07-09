use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, SupportedStreamConfig};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no default input audio device is available")]
    NoInputDevice,
    #[error("failed to enumerate input audio devices: {0}")]
    InputDevices(#[from] cpal::DevicesError),
    #[error("no usable input audio device found: {0}")]
    NoUsableInputDevice(String),
    #[error("microphone permission was denied")]
    MicrophonePermissionDenied,
    #[error("failed to determine microphone permission")]
    MicrophonePermissionUnknown,
    #[error("failed to read default input config: {0}")]
    DefaultConfig(#[from] cpal::DefaultStreamConfigError),
    #[error("failed to enumerate input configs after default config failed: {0}")]
    SupportedConfigs(cpal::SupportedStreamConfigsError),
    #[error("input device did not report any supported configs")]
    NoSupportedInputConfig,
    #[error("failed to build input stream: {0}")]
    BuildStream(#[from] cpal::BuildStreamError),
    #[error("failed to start input stream: {0}")]
    PlayStream(#[from] cpal::PlayStreamError),
    #[error("failed to create WAV writer: {0}")]
    CreateWav(hound::Error),
    #[error("failed to write WAV data: {0}")]
    WriteWav(hound::Error),
    #[error("failed to finalize WAV data: {0}")]
    FinalizeWav(hound::Error),
    #[error("audio device stream error: {0}")]
    Stream(String),
}

pub type AudioResult<T> = Result<T, AudioError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordingInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

struct InputSelection {
    device: cpal::Device,
    name: String,
    config: SupportedStreamConfig,
}

pub fn default_input_device_info() -> AudioResult<InputDeviceInfo> {
    let selection = input_device_config()?;
    let supported = selection.config;

    Ok(InputDeviceInfo {
        name: selection.name,
        sample_rate: supported.sample_rate().0,
        channels: supported.channels(),
        sample_format: format!("{:?}", supported.sample_format()),
    })
}

pub fn record_default_input_to_wav(
    path: impl AsRef<Path>,
    duration: Duration,
) -> AudioResult<RecordingInfo> {
    request_microphone_permission()?;

    let selection = input_device_config()?;
    let device = selection.device;
    let supported = selection.config;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let writer = hound::WavWriter::create(path, spec).map_err(AudioError::CreateWav)?;
    let writer = Arc::new(Mutex::new(Some(writer)));
    let stream_error = Arc::new(Mutex::new(None::<String>));

    let writer_for_stream = Arc::clone(&writer);
    let stream_error_for_callback = Arc::clone(&stream_error);
    let err_fn = move |err: cpal::StreamError| {
        if let Ok(mut slot) = stream_error_for_callback.lock() {
            *slot = Some(err.to_string());
        }
    };

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| write_samples(data.iter().copied(), &writer_for_stream),
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| write_samples(data.iter().copied(), &writer_for_stream),
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| write_samples(data.iter().copied(), &writer_for_stream),
            err_fn,
            None,
        )?,
        other => {
            return Err(AudioError::Stream(format!(
                "unsupported input sample format: {other:?}"
            )));
        }
    };

    stream.play()?;
    std::thread::sleep(duration);
    drop(stream);

    if let Some(err) = stream_error.lock().ok().and_then(|mut slot| slot.take()) {
        return Err(AudioError::Stream(err));
    }

    let writer = writer
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .ok_or_else(|| AudioError::Stream("WAV writer was not available".to_owned()))?;
    writer.finalize().map_err(AudioError::FinalizeWav)?;

    Ok(RecordingInfo {
        sample_rate,
        channels,
        duration,
    })
}

/// Format of the frames emitted by [`stream_default_input_i16`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Handle to a live microphone capture stream. Dropping it stops capture and
/// joins the audio thread.
///
/// `cpal::Stream` is `!Send`, so it cannot be moved across threads or held in an
/// async task. This handle owns a dedicated thread that builds the stream, keeps
/// it alive, and tears it down on drop; PCM frames are delivered to the caller's
/// `on_frame` callback (invoked on cpal's audio thread).
pub struct InputStreamHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InputStreamHandle {
    /// Signal the capture thread to stop and wait for it to finish.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl Drop for InputStreamHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Open the default input device and stream mono 16-bit PCM frames to
/// `on_frame` until the returned [`InputStreamHandle`] is dropped.
///
/// Frames are delivered as mono PCM at the device's native sample rate
/// (reported in [`InputStreamInfo`]); the caller is responsible for any
/// resampling. Multi-channel input is downmixed to mono. The callback runs on
/// cpal's realtime audio thread, so it must not block.
pub fn stream_default_input_i16(
    on_frame: impl FnMut(&[i16]) + Send + 'static,
) -> AudioResult<(InputStreamHandle, InputStreamInfo)> {
    request_microphone_permission()?;

    let selection = input_device_config()?;
    let device = selection.device;
    let supported = selection.config;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    let channel_count = channels.max(1) as usize;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel::<AudioResult<()>>();

    let thread = thread::spawn(move || {
        let mut on_frame = on_frame;
        let stop_on_error = Arc::clone(&stop_thread);
        let err_fn = move |_err: cpal::StreamError| {
            stop_on_error.store(true, Ordering::SeqCst);
        };

        // Build + start the stream. Each arm moves `on_frame`/`err_fn`; arms are
        // mutually exclusive so the multiple moves are sound.
        let built = (|| -> AudioResult<cpal::Stream> {
            let stream = match sample_format {
                SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        let mono = downmix_to_i16(data, channel_count);
                        if !mono.is_empty() {
                            on_frame(&mono);
                        }
                    },
                    err_fn,
                    None,
                )?,
                SampleFormat::I16 => device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        let mono = downmix_to_i16(data, channel_count);
                        if !mono.is_empty() {
                            on_frame(&mono);
                        }
                    },
                    err_fn,
                    None,
                )?,
                SampleFormat::U16 => device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        let mono = downmix_to_i16(data, channel_count);
                        if !mono.is_empty() {
                            on_frame(&mono);
                        }
                    },
                    err_fn,
                    None,
                )?,
                other => {
                    return Err(AudioError::Stream(format!(
                        "unsupported input sample format: {other:?}"
                    )));
                }
            };
            stream.play()?;
            Ok(stream)
        })();

        match built {
            Ok(stream) => {
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                while !stop_thread.load(Ordering::SeqCst) {
                    thread::park_timeout(Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(err) => {
                let _ = ready_tx.send(Err(err));
            }
        }
    });

    match ready_rx.recv() {
        Ok(Ok(())) => Ok((
            InputStreamHandle {
                stop,
                thread: Some(thread),
            },
            InputStreamInfo {
                sample_rate,
                channels: 1,
            },
        )),
        Ok(Err(err)) => {
            let _ = thread.join();
            Err(err)
        }
        Err(_) => {
            let _ = thread.join();
            Err(AudioError::Stream(
                "input stream thread exited during setup".to_owned(),
            ))
        }
    }
}

/// Downmix an interleaved multi-channel buffer to mono `i16`.
fn downmix_to_i16<S>(data: &[S], channels: usize) -> Vec<i16>
where
    S: IntoSampleI16 + Copy,
{
    if channels <= 1 {
        return data.iter().map(|s| s.into_i16()).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|s| s.into_i16() as i32).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}

fn input_device_config() -> AudioResult<InputSelection> {
    let host = cpal::default_host();
    let mut errors = Vec::new();

    if let Some(device) = host.default_input_device() {
        match input_selection(device) {
            Ok(selection) => return Ok(selection),
            Err(err) => errors.push(format!("default input: {err}")),
        }
    } else {
        errors.push(AudioError::NoInputDevice.to_string());
    }

    for device in host.input_devices().map_err(AudioError::InputDevices)? {
        match input_selection(device) {
            Ok(selection) => return Ok(selection),
            Err(err) => errors.push(err.to_string()),
        }
    }

    Err(AudioError::NoUsableInputDevice(errors.join("; ")))
}

fn input_selection(device: cpal::Device) -> AudioResult<InputSelection> {
    let name = device
        .name()
        .unwrap_or_else(|_| "(unknown input device)".to_owned());
    let config = input_stream_config(&device)?;
    Ok(InputSelection {
        device,
        name,
        config,
    })
}

fn input_stream_config(device: &cpal::Device) -> AudioResult<SupportedStreamConfig> {
    if let Ok(config) = device.default_input_config() {
        return Ok(config);
    }

    let config = device
        .supported_input_configs()
        .map_err(AudioError::SupportedConfigs)?
        .max_by(|a, b| a.cmp_default_heuristics(b))
        .ok_or(AudioError::NoSupportedInputConfig)?;

    Ok(config
        .clone()
        .try_with_sample_rate(SampleRate(44_100))
        .unwrap_or_else(|| config.with_max_sample_rate()))
}

#[cfg(target_os = "macos")]
fn request_microphone_permission() -> AudioResult<()> {
    unsafe extern "C" {
        fn dicta_audio_request_microphone_permission() -> std::os::raw::c_int;
    }

    match unsafe { dicta_audio_request_microphone_permission() } {
        0 => Ok(()),
        1 => Err(AudioError::MicrophonePermissionDenied),
        _ => Err(AudioError::MicrophonePermissionUnknown),
    }
}

#[cfg(not(target_os = "macos"))]
fn request_microphone_permission() -> AudioResult<()> {
    Ok(())
}

fn write_samples<S>(
    samples: impl Iterator<Item = S>,
    writer: &Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
) where
    S: IntoSampleI16,
{
    let Ok(mut guard) = writer.lock() else {
        return;
    };
    let Some(writer) = guard.as_mut() else {
        return;
    };

    for sample in samples {
        if writer.write_sample(sample.into_i16()).is_err() {
            break;
        }
    }
}

trait IntoSampleI16 {
    fn into_i16(self) -> i16;
}

impl IntoSampleI16 for f32 {
    fn into_i16(self) -> i16 {
        let clamped = self.clamp(-1.0, 1.0);
        (clamped * i16::MAX as f32) as i16
    }
}

impl IntoSampleI16 for i16 {
    fn into_i16(self) -> i16 {
        self
    }
}

impl IntoSampleI16 for u16 {
    fn into_i16(self) -> i16 {
        (self as i32 - 32768) as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_conversion_clamps_float_values() {
        assert_eq!(2.0_f32.into_i16(), i16::MAX);
        assert_eq!((-2.0_f32).into_i16(), i16::MIN + 1);
        assert_eq!(0.0_f32.into_i16(), 0);
    }

    #[test]
    fn sample_conversion_centers_unsigned_values() {
        assert_eq!(0_u16.into_i16(), i16::MIN);
        assert_eq!(32768_u16.into_i16(), 0);
        assert_eq!(u16::MAX.into_i16(), i16::MAX);
    }

    #[test]
    fn downmixes_multichannel_audio_to_mono() {
        assert_eq!(
            downmix_to_i16(&[100_i16, 300, -100, -300], 2),
            vec![200, -200]
        );
    }
}
