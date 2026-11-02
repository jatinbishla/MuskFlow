//! Microphone capture.
//!
//! The cpal callback runs on a real-time audio thread, so it only downmixes to
//! mono and pushes into a lock-free ring buffer: no allocation, no locks. A pump
//! thread drains the ring every 10 ms, resamples to 16 kHz and forwards chunks
//! over a channel to whoever is listening (the ASR committer).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use crossbeam_channel::Sender;
use sherpa_onnx::LinearResampler;

use crate::SAMPLE_RATE;

/// 30 s of headroom at 48 kHz; the pump drains every 10 ms so this never fills in practice.
const RING_CAPACITY: usize = 48_000 * 30;
const PUMP_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone)]
pub struct MicInfo {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// A running microphone capture. Dropping it (or calling [`MicCapture::stop`])
/// closes the device and flushes the resampler.
pub struct MicCapture {
    // cpal streams are !Send on some platforms, so the stream stays on the thread that made it.
    stream: Option<cpal::Stream>,
    running: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    level: Arc<AtomicU32>,
    pub info: MicInfo,
}

impl MicCapture {
    /// Opens the default input device and starts streaming 16 kHz mono chunks to `tx`.
    pub fn start(tx: Sender<Vec<f32>>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| {
            anyhow!("no input device found (is a microphone connected and allowed?)")
        })?;
        let config = device
            .default_input_config()
            .context("could not read the input device's default config")?;

        let info = MicInfo {
            device: device
                .id()
                .map(|id| id.to_string())
                .unwrap_or_else(|_| "unknown".into()),
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };

        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(RING_CAPACITY);
        let level = Arc::new(AtomicU32::new(0));
        let channels = info.channels as usize;

        let err_fn = |err| log::warn!("audio stream error: {err}");
        let stream_config: cpal::StreamConfig = config.into();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(
                &device,
                &stream_config,
                channels,
                producer,
                level.clone(),
                err_fn,
            ),
            SampleFormat::I16 => build::<i16>(
                &device,
                &stream_config,
                channels,
                producer,
                level.clone(),
                err_fn,
            ),
            SampleFormat::I32 => build::<i32>(
                &device,
                &stream_config,
                channels,
                producer,
                level.clone(),
                err_fn,
            ),
            SampleFormat::U16 => build::<u16>(
                &device,
                &stream_config,
                channels,
                producer,
                level.clone(),
                err_fn,
            ),
            other => return Err(anyhow!("unsupported sample format {other}")),
        }?;
        stream.play().context("could not start the input stream")?;

        let running = Arc::new(AtomicBool::new(true));
        let pump = spawn_pump(consumer, info.sample_rate, running.clone(), tx)?;

        Ok(Self {
            stream: Some(stream),
            running,
            pump: Some(pump),
            level,
            info,
        })
    }

    /// Current input level (RMS of the latest callback buffer), 0.0–1.0, for the waveform UI.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Stops capture. Audio captured before this call is still delivered, then the channel closes.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Drop the stream first so no more samples arrive, then let the pump drain and flush.
        self.stream.take();
        self.running.store(false, Ordering::Release);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut producer: rtrb::Producer<f32>,
    level: Arc<AtomicU32>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut sum_sq = 0.0f32;
            let mut frames = 0usize;
            for frame in data.chunks_exact(channels) {
                let mono =
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                sum_sq += mono * mono;
                frames += 1;
                // If the ring is full the consumer has stalled; dropping samples beats blocking here.
                let _ = producer.push(mono);
            }
            if frames > 0 {
                level.store((sum_sq / frames as f32).sqrt().to_bits(), Ordering::Relaxed);
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

fn spawn_pump(
    mut consumer: rtrb::Consumer<f32>,
    input_rate: u32,
    running: Arc<AtomicBool>,
    tx: Sender<Vec<f32>>,
) -> Result<JoinHandle<()>> {
    let resampler = if input_rate == SAMPLE_RATE {
        None
    } else {
        Some(
            LinearResampler::create(input_rate as i32, SAMPLE_RATE as i32)
                .ok_or_else(|| anyhow!("could not create a {input_rate} Hz → 16 kHz resampler"))?,
        )
    };

    let handle = std::thread::Builder::new()
        .name("mf-audio-pump".into())
        .spawn(move || {
            let mut buf = Vec::with_capacity(input_rate as usize / 10);
            loop {
                let stopping = !running.load(Ordering::Acquire);
                buf.clear();
                let available = consumer.slots();
                if available > 0
                    && let Ok(chunk) = consumer.read_chunk(available)
                {
                    let (a, b) = chunk.as_slices();
                    buf.extend_from_slice(a);
                    buf.extend_from_slice(b);
                    chunk.commit_all();
                }
                if !buf.is_empty() || stopping {
                    let out = match &resampler {
                        Some(r) => r.resample(&buf, stopping),
                        None => buf.clone(),
                    };
                    if !out.is_empty() && tx.send(out).is_err() {
                        return; // listener went away
                    }
                }
                if stopping {
                    return; // dropping tx closes the channel
                }
                std::thread::sleep(PUMP_INTERVAL);
            }
        })?;
    Ok(handle)
}
