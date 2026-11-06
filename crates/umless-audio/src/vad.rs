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

fn create_vad(
    model: &Path,
    cfg: &VadConfig,
    max_speech_secs: f32,
) -> Result<VoiceActivityDetector> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            threshold: cfg.threshold,
            min_silence_duration: cfg.min_silence_secs,
            min_speech_duration: cfg.min_speech_secs,
            window_size: WINDOW as i32,
            max_speech_duration: max_speech_secs,
        },
        sample_rate: SAMPLE_RATE as i32,
        num_threads: 1,
        ..Default::default()
    };
    VoiceActivityDetector::create(&config, 330.0)
        .ok_or_else(|| anyhow!("could not load Silero VAD from {}", model.display()))
}

/// A finished stretch of speech, cut by VAD at a pause.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Start, in samples since the stream began.
    pub start: usize,
    pub samples: Vec<f32>,
}

/// Incremental VAD: feed audio as it arrives, get back each phrase once the speaker pauses.
pub struct SpeechSegmenter {
    vad: VoiceActivityDetector,
    pending: Vec<f32>,
}

impl SpeechSegmenter {
    /// `max_phrase_secs` force-splits speech that never pauses, so no single decode gets too long.
    pub fn load(model: &Path, cfg: VadConfig, max_phrase_secs: f32) -> Result<Self> {
        Ok(Self {
            vad: create_vad(model, &cfg, max_phrase_secs)?,
            pending: Vec::with_capacity(WINDOW),
        })
    }

    /// Feeds audio; returns phrases that ended during this call.
    pub fn push(&mut self, pcm: &[f32]) -> Vec<SpeechSegment> {
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / WINDOW * WINDOW;
        for window in self.pending[..whole].chunks(WINDOW) {
            self.vad.accept_waveform(window);
        }
        self.pending.drain(..whole);
        self.drain()
    }

    /// End of input: returns whatever speech is still open.
    pub fn flush(&mut self) -> Vec<SpeechSegment> {
        if !self.pending.is_empty() {
            let mut last = std::mem::take(&mut self.pending);
            last.resize(WINDOW, 0.0);
            self.vad.accept_waveform(&last);
        }
        self.vad.flush();
        self.drain()
    }

    /// Whether the speaker is talking right now.
    pub fn in_speech(&self) -> bool {
        self.vad.detected()
    }

    fn drain(&mut self) -> Vec<SpeechSegment> {
        let mut out = Vec::new();
        while let Some(seg) = self.vad.front() {
            out.push(SpeechSegment {
                start: seg.start().max(0) as usize,
                samples: seg.samples().to_vec(),
            });
            self.vad.pop();
        }
        out
    }
}

pub struct SpeechTrimmer {
    vad: VoiceActivityDetector,
    pad: usize,
}

impl SpeechTrimmer {
    pub fn load(model: &Path, cfg: VadConfig) -> Result<Self> {
        let vad = create_vad(model, &cfg, 300.0)?;
        Ok(Self {
            vad,
            pad: secs_to_samples(cfg.pad_secs),
        })
    }

    /// Every stretch of speech in the clip, as `[start, end)` sample ranges in order.
    pub fn speech_segments(&self, pcm: &[f32]) -> Vec<(usize, usize)> {
        self.vad.reset();
        let mut out = Vec::new();
        let mut collect = |vad: &VoiceActivityDetector| {
            while let Some(seg) = vad.front() {
                let start = seg.start().max(0) as usize;
                out.push((start, start + seg.n().max(0) as usize));
                vad.pop();
            }
        };
        for window in pcm.chunks(WINDOW) {
            self.vad.accept_waveform(window);
            collect(&self.vad);
        }
        self.vad.flush();
        collect(&self.vad);
        out
    }

    /// Returns the `[start, end)` sample range that contains speech (with padding),
    /// or `None` if the clip has no speech at all.
    pub fn speech_bounds(&self, pcm: &[f32]) -> Option<(usize, usize)> {
        let segments = self.speech_segments(pcm);
        let start = segments.first()?.0;
        let last = segments.iter().map(|s| s.1).max().unwrap_or(start);
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
