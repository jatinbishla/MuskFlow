use std::path::Path;

use anyhow::{Context, Result, anyhow};
use sherpa_onnx::LinearResampler;

use crate::SAMPLE_RATE;

/// Loads any PCM WAV (int or float, any rate, any channel count) as 16 kHz mono f32.
pub fn load_wav_16k(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    if spec.sample_rate == SAMPLE_RATE {
        return Ok(mono);
    }
    let resampler = LinearResampler::create(spec.sample_rate as i32, SAMPLE_RATE as i32)
        .ok_or_else(|| {
            anyhow!(
                "could not create a {} Hz → 16 kHz resampler",
                spec.sample_rate
            )
        })?;
    Ok(resampler.resample(&mono, true))
}
