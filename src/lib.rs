use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub mod recorder;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct ManagedStream(cpal::Stream);

impl Drop for ManagedStream {
    fn drop(&mut self) {
        if let Err(error) = self.0.pause() {
            eprintln!("Could not pause audio stream: {error}");
        }
    }
}

#[derive(Default)]
struct Metrics {
    clipped: AtomicU64,
    overflow: AtomicU64,
    underrun: AtomicU64,
    errors: AtomicU64,
    peak: AtomicU64,
}

impl Metrics {
    fn readings(&self) -> Readings {
        Readings {
            peak: self.peak.swap(0, Ordering::Relaxed) as f32 / 1_000_000.0,
            clipped: self.clipped.load(Ordering::Relaxed),
            overflow: self.overflow.load(Ordering::Relaxed),
            underrun: self.underrun.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

fn configuration(device: &cpal::Device, input: bool) -> Result<cpal::StreamConfig> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        match device.supported_output_configs() {
            Ok(ranges) => ranges.collect(),
            #[cfg(target_os = "macos")]
            Err(_) if output_device_names()?.contains(&device.name()?) => {
                return Ok(cpal::StreamConfig {
                    channels: 2,
                    sample_rate: cpal::SampleRate(48_000),
                    buffer_size: cpal::BufferSize::Default,
                });
            }
            Err(error) => return Err(error.into()),
        }
    };
    let range = ranges
        .into_iter()
        .find(|range| {
            range.channels() == 2
                && range.sample_format() == cpal::SampleFormat::F32
                && range.min_sample_rate().0 <= 48_000
                && range.max_sample_rate().0 >= 48_000
        })
        .ok_or("Device must support stereo f32 at 48000 Hz")?;
    Ok(range.with_sample_rate(cpal::SampleRate(48_000)).config())
}

#[derive(Default, Clone, Copy)]
pub struct Readings {
    pub peak: f32,
    pub clipped: u64,
    pub overflow: u64,
    pub underrun: u64,
    pub errors: u64,
}

pub struct Devices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_output: Option<String>,
}

#[cfg(target_os = "macos")]
fn output_device_names() -> Result<Vec<String>> {
    use coreaudio::audio_unit::macos_helpers::{
        get_audio_device_ids, get_audio_device_supports_scope, get_device_name,
    };
    use coreaudio::audio_unit::Scope;

    let mut names = Vec::new();
    for device_id in get_audio_device_ids()? {
        if get_audio_device_supports_scope(device_id, Scope::Output).unwrap_or(false) {
            names.push(get_device_name(device_id)?);
        }
    }
    Ok(names)
}

#[cfg(not(target_os = "macos"))]
fn output_device_names() -> Result<Vec<String>> {
    Ok(cpal::default_host()
        .output_devices()?
        .map(|device| device.name())
        .collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn devices() -> Result<Devices> {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()?
        .map(|device| device.name())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let outputs = output_device_names()?;
    let default_output = host
        .default_output_device()
        .and_then(|device| device.name().ok());
    Ok(Devices {
        inputs,
        outputs,
        default_output,
    })
}

pub struct Monitor {
    _output_stream: ManagedStream,
    _input_stream: ManagedStream,
    metrics: Arc<Metrics>,
    gain: Arc<AtomicU32>,
    active: Arc<AtomicBool>,
}

impl Monitor {
    pub fn start(
        input_name: &str,
        output_name: &str,
        gain_db: f32,
        latency_ms: usize,
    ) -> Result<Self> {
        if !gain_db.is_finite() || !(-60.0..=12.0).contains(&gain_db) {
            return Err("Gain must be between -60 and +12 dB".into());
        }
        Self::start_with_gain(
            input_name,
            output_name,
            10.0_f32.powf(gain_db / 20.0),
            latency_ms,
        )
    }

    pub fn start_with_volume(
        input_name: &str,
        output_name: &str,
        volume_percent: f32,
        latency_ms: usize,
    ) -> Result<Self> {
        if !volume_percent.is_finite() || !(0.0..=100.0).contains(&volume_percent) {
            return Err("Volume must be between 0 and 100 percent".into());
        }
        Self::start_with_gain(input_name, output_name, volume_percent / 100.0, latency_ms)
    }

    fn start_with_gain(
        input_name: &str,
        output_name: &str,
        gain_factor: f32,
        latency_ms: usize,
    ) -> Result<Self> {
        let host = cpal::default_host();
        if !(50..=1000).contains(&latency_ms) {
            return Err("Latency must be between 50 and 1000 ms".into());
        }
        let input = host
            .input_devices()?
            .find(|device| device.name().ok().as_deref() == Some(input_name))
            .ok_or("Input not found; refresh devices or use --list")?;
        let output = host
            .devices()?
            .find(|device| device.name().ok().as_deref() == Some(output_name))
            .ok_or("Output not found; refresh devices or use --list")?;
        if input_name == output_name {
            return Err("Choose speakers or headphones as output, not the DM13 itself".into());
        }
        let input_config = configuration(&input, true)?;
        let output_config = configuration(&output, false)?;
        let target_frames = 48 * latency_ms;
        let ring = HeapRb::<[f32; 2]>::new(target_frames * 4);
        let (mut producer, mut consumer) = ring.split();
        let metrics = Arc::new(Metrics::default());
        let active = Arc::new(AtomicBool::new(true));
        let input_active = active.clone();
        let output_active = active.clone();
        let input_metrics = metrics.clone();
        let input_errors = metrics.clone();
        let input_stream = ManagedStream(input.build_input_stream(
            &input_config,
            move |samples: &[f32], _: &cpal::InputCallbackInfo| {
                if !input_active.load(Ordering::Acquire) {
                    return;
                }
                for frame in samples.chunks_exact(2) {
                    let peak = frame[0].abs().max(frame[1].abs());
                    if peak >= 0.9999 {
                        input_metrics.clipped.fetch_add(1, Ordering::Relaxed);
                    }
                    input_metrics
                        .peak
                        .fetch_max((peak * 1_000_000.0) as u64, Ordering::Relaxed);
                    if producer.push([frame[0], frame[1]]).is_err() {
                        input_metrics.overflow.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
            move |_| {
                input_errors.errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?);
        let output_metrics = metrics.clone();
        let output_errors = metrics.clone();
        let gain = Arc::new(AtomicU32::new(gain_factor.to_bits()));
        let output_gain = gain.clone();
        let mut current_gain = f32::from_bits(gain.load(Ordering::Relaxed));
        let mut primed = false;
        let mut previous = [0.0_f32; 2];
        let mut next = [0.0_f32; 2];
        let mut position = 0.0_f64;
        let output_stream = ManagedStream(output.build_output_stream(
            &output_config,
            move |samples: &mut [f32], _: &cpal::OutputCallbackInfo| {
                samples.fill(0.0);
                if !output_active.load(Ordering::Acquire) {
                    return;
                }
                if !primed {
                    if consumer.len() < target_frames {
                        return;
                    }
                    previous = consumer.pop().unwrap_or([0.0; 2]);
                    next = consumer.pop().unwrap_or([0.0; 2]);
                    position = 0.0;
                    primed = true;
                }
                let error = (consumer.len() as f64 - target_frames as f64) / target_frames as f64;
                let ratio = 1.0 + (error * 0.002).clamp(-0.005, 0.005);
                let target_gain = f32::from_bits(output_gain.load(Ordering::Relaxed));
                for frame in samples.chunks_exact_mut(2) {
                    while position >= 1.0 {
                        previous = next;
                        match consumer.pop() {
                            Some(value) => next = value,
                            None => {
                                output_metrics.underrun.fetch_add(1, Ordering::Relaxed);
                                primed = false;
                                return;
                            }
                        }
                        position -= 1.0;
                    }
                    current_gain += (target_gain - current_gain) * 0.002;
                    let applied_gain = if target_gain == 0.0 {
                        0.0
                    } else {
                        current_gain
                    };
                    for channel in 0..2 {
                        let value = previous[channel]
                            + (next[channel] - previous[channel]) * position as f32;
                        frame[channel] = (value * applied_gain).clamp(-1.0, 1.0);
                    }
                    position += ratio;
                }
            },
            move |_| {
                output_errors.errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?);
        let monitor = Self {
            _output_stream: output_stream,
            _input_stream: input_stream,
            metrics,
            gain,
            active,
        };
        monitor._input_stream.0.play()?;
        monitor._output_stream.0.play()?;
        Ok(monitor)
    }

    pub fn set_gain_db(&self, gain_db: f32) {
        if gain_db.is_finite() {
            self.gain.store(
                10.0_f32.powf(gain_db.clamp(-60.0, 12.0) / 20.0).to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    pub fn set_volume_percent(&self, volume_percent: f32) {
        if volume_percent.is_finite() {
            self.gain.store(
                (volume_percent.clamp(0.0, 100.0) / 100.0).to_bits(),
                Ordering::Relaxed,
            );
        }
    }

    pub fn readings(&self) -> Readings {
        self.metrics.readings()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}
