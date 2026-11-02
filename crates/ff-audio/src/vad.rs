//! Voice activity detection (Silero VAD via sherpa-onnx).
//!
//! Used to trim leading/trailing silence before decoding (less audio means
//! faster ASR), to drop sessions with no speech, and later for hands-free
//! auto-stop.

use std::path::Path;

use anyhow::{Result, anyhow};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use crate::{SAMPLE_RATE, secs_to_samples};

/// Silero processes fixed 512-sample windows at 16 kHz (32 ms).
const WINDOW: usize = 512;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub threshold: f32,
    pub min_silence_secs: f32,
    pub min_speech_secs: f32,
    /// Audio kept before the first and after the last speech segment, so word edges are not clipped.
    pub pad_secs: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_silence_secs: 0.3,
            min_speech_secs: 0.15,
            pad_secs: 0.25,
        }
    }
}

pub struct SpeechTrimmer {
    vad: VoiceActivityDetector,
    pad: usize,
}

impl SpeechTrimmer {
    pub fn load(model: &Path, cfg: VadConfig) -> Result<Self> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model.to_string_lossy().into_owned()),
                threshold: cfg.threshold,
                min_silence_duration: cfg.min_silence_secs,
                min_speech_duration: cfg.min_speech_secs,
                window_size: WINDOW as i32,
                max_speech_duration: 300.0,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&config, 330.0)
            .ok_or_else(|| anyhow!("could not load Silero VAD from {}", model.display()))?;
        Ok(Self {
            vad,
            pad: secs_to_samples(cfg.pad_secs),
        })
    }

    /// Returns the `[start, end)` sample range that contains speech (with padding),
    /// or `None` if the clip has no speech at all.
    pub fn speech_bounds(&self, pcm: &[f32]) -> Option<(usize, usize)> {
        self.vad.reset();
        let mut first: Option<usize> = None;
        let mut last = 0usize;
        let mut collect = |vad: &VoiceActivityDetector| {
            while let Some(seg) = vad.front() {
                let start = seg.start().max(0) as usize;
                first.get_or_insert(start);
                last = last.max(start + seg.n().max(0) as usize);
                vad.pop();
            }
        };
        for window in pcm.chunks(WINDOW) {
            self.vad.accept_waveform(window);
            collect(&self.vad);
        }
        self.vad.flush();
        collect(&self.vad);

        let start = first?;
        Some((
            start.saturating_sub(self.pad),
            (last + self.pad).min(pcm.len()),
        ))
    }

    /// Trims silence; returns an empty slice when there is no speech.
    pub fn trim<'a>(&self, pcm: &'a [f32]) -> &'a [f32] {
        match self.speech_bounds(pcm) {
            Some((s, e)) => &pcm[s..e],
            None => &[],
        }
    }
}
