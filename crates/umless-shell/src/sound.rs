//! Short synthesized tones for start / finish / error on Windows and Linux (macOS uses
//! its system sounds). Played on a background thread through the default output device.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Plays `(frequency Hz, milliseconds)` notes one after another, quietly. Never blocks.
pub fn play(notes: &'static [(f32, u32)]) {
    std::thread::spawn(move || {
        if let Err(e) = play_blocking(notes) {
            log::debug!("no sound: {e:#}");
        }
    });
}

fn play_blocking(notes: &[(f32, u32)]) -> anyhow::Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output device"))?;
    let config = device.default_output_config()?;
    let rate = config.sample_rate() as f32;
    let channels = config.channels() as usize;
    // The whole sound, rendered up front with a short fade at each note's ends.
    let mut samples = Vec::new();
    for &(freq, ms) in notes {
        let n = (rate * ms as f32 / 1000.0) as usize;
        let fade = (rate * 0.006) as usize;
        for i in 0..n {
            let env = (i.min(n - i) as f32 / fade as f32).min(1.0);
            let v = (i as f32 * freq * std::f32::consts::TAU / rate).sin() * 0.12 * env;
            samples.push(v);
        }
    }
    let total_ms: u32 = notes.iter().map(|n| n.1).sum();
    let mut pos = 0usize;
    let stream_config: cpal::StreamConfig = config.config();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_output_stream(
            stream_config.clone(),
            move |out: &mut [f32], _| {
                for frame in out.chunks_mut(channels) {
                    let v = samples.get(pos).copied().unwrap_or(0.0);
                    pos += 1;
                    frame.iter_mut().for_each(|s| *s = v);
                }
            },
            |e| log::debug!("sound: {e}"),
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            stream_config.clone(),
            move |out: &mut [i16], _| {
                for frame in out.chunks_mut(channels) {
                    let v = samples.get(pos).copied().unwrap_or(0.0);
                    pos += 1;
                    frame
                        .iter_mut()
                        .for_each(|s| *s = (v * i16::MAX as f32) as i16);
                }
            },
            |e| log::debug!("sound: {e}"),
            None,
        )?,
        other => anyhow::bail!("unsupported output format {other:?}"),
    };
    stream.play()?;
    std::thread::sleep(std::time::Duration::from_millis(total_ms as u64 + 80));
    Ok(())
}
