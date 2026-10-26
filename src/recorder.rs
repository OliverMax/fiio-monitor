use crate::{configuration, ManagedStream, Metrics, Readings, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct RecordingSummary {
    pub path: PathBuf,
    pub frames: u64,
    pub warning: Option<String>,
}

impl RecordingSummary {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames as f64 / 48_000.0)
    }
}

pub struct Recorder {
    stream: Option<ManagedStream>,
    worker: Option<JoinHandle<std::result::Result<RecordingSummary, String>>>,
    stopped: Arc<AtomicBool>,
    frames_written: Arc<AtomicU64>,
    metrics: Arc<Metrics>,
}

impl Recorder {
    pub fn start(input_name: &str, path: &Path) -> Result<Self> {
        let input = cpal::default_host()
            .input_devices()?
            .find(|device| device.name().ok().as_deref() == Some(input_name))
            .ok_or("Input not found; refresh devices")?;
        let config = configuration(&input, true)?;
        let metrics = Arc::new(Metrics::default());
        let input_metrics = metrics.clone();
        let input_errors = metrics.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let input_stopped = stopped.clone();
        let ring = HeapRb::<[f32; 2]>::new(48_000 * 2);
        let (mut producer, mut consumer) = ring.split();
        let stream = ManagedStream(input.build_input_stream(
            &config,
            move |samples: &[f32], _: &cpal::InputCallbackInfo| {
                if input_stopped.load(Ordering::Acquire)
                    || input_metrics.errors.load(Ordering::Relaxed) > 0
                {
                    return;
                }
                for frame in samples.chunks_exact(2) {
                    if !frame[0].is_finite() || !frame[1].is_finite() {
                        input_metrics.errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    let peak = frame[0].abs().max(frame[1].abs());
                    input_metrics
                        .peak
                        .fetch_max((peak * 1_000_000.0) as u64, Ordering::Relaxed);
                    if peak >= 0.9999 {
                        input_metrics.clipped.fetch_add(1, Ordering::Relaxed);
                    }
                    if producer.push([frame[0], frame[1]]).is_err() {
                        input_metrics.overflow.fetch_add(1, Ordering::Relaxed);
                        input_metrics.errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            },
            move |_| {
                input_errors.errors.fetch_add(1, Ordering::Relaxed);
            },
            None,
        )?);
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let specification = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(BufWriter::new(file), specification)?;
        let worker_stopped = stopped.clone();
        let frames_written = Arc::new(AtomicU64::new(0));
        let worker_frames = frames_written.clone();
        let worker_metrics = metrics.clone();
        let path = path.to_owned();
        let worker = thread::Builder::new().name("fiio-wav-writer".to_owned()).spawn(move || {
            let mut frames = 0_u64;
            let mut warning = None;
            'recording: loop {
                while let Some(frame) = consumer.pop() {
                    if frames >= (u32::MAX as u64 - 1024) / 4 {
                        warning = Some("Reached the WAV file size limit; recording stopped.".to_owned());
                        worker_metrics.errors.fetch_add(1, Ordering::Relaxed);
                        break 'recording;
                    }
                    for sample in frame {
                        let sample = (sample.clamp(-1.0, 1.0) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                        if let Err(error) = writer.write_sample(sample) {
                            worker_metrics.errors.fetch_add(1, Ordering::Relaxed);
                            let finalization = writer.finalize();
                            return Err(format!("Write failed for {}: {error}. File may be incomplete. Finalization: {finalization:?}", path.display()));
                        }
                    }
                    frames += 1;
                    worker_frames.store(frames, Ordering::Relaxed);
                }
                if worker_stopped.load(Ordering::Acquire) || worker_metrics.errors.load(Ordering::Relaxed) > 0 {
                    if consumer.is_empty() { break; }
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            writer.finalize().map_err(|error| format!("Could not finalize {}: {error}. File may be incomplete.", path.display()))?;
            if warning.is_none() && worker_metrics.errors.load(Ordering::Relaxed) > 0 {
                warning = Some("Recording interrupted by an audio or buffer error; the captured portion was saved.".to_owned());
            }
            Ok(RecordingSummary { path, frames, warning })
        })?;
        let recorder = Self {
            stream: Some(stream),
            worker: Some(worker),
            stopped,
            frames_written,
            metrics,
        };
        recorder
            .stream
            .as_ref()
            .ok_or("Input stream unavailable")?
            .0
            .play()?;
        Ok(recorder)
    }

    pub fn readings(&self) -> Readings {
        self.metrics.readings()
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames_written.load(Ordering::Relaxed) as f64 / 48_000.0)
    }

    fn finish(&mut self) -> Result<RecordingSummary> {
        self.stopped.store(true, Ordering::Release);
        self.stream.take();
        let worker = self.worker.take().ok_or("Recording already stopped")?;
        match worker.join() {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err("Recording worker failed; the WAV file may be incomplete".into()),
        }
    }

    pub fn stop(mut self) -> Result<RecordingSummary> {
        self.finish()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.worker.is_some() {
            if let Err(error) = self.finish() {
                eprintln!("{error}");
            }
        }
    }
}
