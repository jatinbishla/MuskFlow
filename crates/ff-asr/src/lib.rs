//! Speech recognition for FreeFlow.
//!
//! [`Transcriber`] is the backend-agnostic interface; [`Committer`] turns any
//! offline transcriber into a streaming one by locking in words as soon as two
//! consecutive decodes agree on them (LocalAgreement-2), so that on key-up only
//! the last few seconds of audio are left to decode.

mod committer;
mod parakeet;
mod streaming;

pub use committer::{Committer, CommitterConfig};
pub use parakeet::{ParakeetConfig, ParakeetTranscriber};
pub use streaming::{StreamingConfig, StreamingParakeet};

use std::time::Duration;

use anyhow::Result;

/// One recognised word with absolute timestamps in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f32,
    pub end: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hypothesis {
    pub words: Vec<Word>,
}

impl Hypothesis {
    pub fn text(&self) -> String {
        join_words(&self.words)
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

pub fn join_words(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

pub trait Transcriber: Send + Sync {
    /// Short identifier for logs and benchmarks, e.g. `parakeet-tdt-0.6b-v3-int8`.
    fn name(&self) -> &str;

    /// Decodes 16 kHz mono PCM. Word timestamps are relative to the start of `pcm`.
    /// `hotwords` are boosted terms from screen context and the user dictionary;
    /// backends that cannot bias ignore them.
    fn transcribe(&self, pcm: &[f32], hotwords: &[String]) -> Result<Hypothesis>;
}

/// One dictation, from key-down to key-up. Implemented by [`Committer`] (offline
/// model + LocalAgreement) and by native streaming backends.
pub trait Session {
    /// Adds 16 kHz mono audio. Returns words that became final during this call.
    fn push(&mut self, pcm: &[f32]) -> Result<Vec<Word>>;
    /// Adds audio without decoding. Used after key-up, when the next decode is `finish`.
    fn append(&mut self, pcm: &[f32]);
    /// `(final, tentative)` text for live display.
    fn partial_text(&self) -> (String, String);
    fn audio_secs(&self) -> f32;
    /// Key-up: decodes whatever is left and returns the whole transcript.
    fn finish(self: Box<Self>) -> Result<(Hypothesis, SessionStats)>;
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub decodes: usize,
    pub decode_time: Duration,
    /// Audio still undecoded at key-up, in seconds.
    pub tail_secs: f32,
    /// Time spent finishing after key-up (the ASR share of key-up latency).
    pub tail_time: Duration,
}

/// A loaded ASR model that can start dictation sessions.
pub enum Engine {
    Offline {
        asr: ParakeetTranscriber,
        hotwords: Vec<String>,
        cfg: CommitterConfig,
    },
    Streaming(StreamingParakeet),
}

impl Engine {
    /// Loads the model in `model_dir`; directories with "streaming" in the name use the native streaming backend.
    pub fn load(
        model_dir: &std::path::Path,
        num_threads: i32,
        provider: &str,
        hotwords: Vec<String>,
    ) -> Result<Self> {
        let is_streaming = model_dir.to_string_lossy().contains("streaming");
        if is_streaming {
            let mut cfg = StreamingConfig::new(model_dir);
            cfg.num_threads = num_threads;
            cfg.provider = provider.into();
            return Ok(Self::Streaming(StreamingParakeet::load(&cfg)?));
        }
        let mut cfg = ParakeetConfig::new(model_dir);
        cfg.num_threads = num_threads;
        cfg.provider = provider.into();
        cfg.hotwords = !hotwords.is_empty();
        Ok(Self::Offline {
            asr: ParakeetTranscriber::load(&cfg)?,
            hotwords,
            cfg: CommitterConfig::default(),
        })
    }

    /// Tunes how often an offline model re-decodes while the user speaks. No-op for streaming models.
    pub fn with_committer(mut self, new: CommitterConfig) -> Self {
        if let Self::Offline { cfg, .. } = &mut self {
            *cfg = new;
        }
        self
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Offline { asr, .. } => asr.name(),
            Self::Streaming(s) => s.name(),
        }
    }

    pub fn session(&self) -> Box<dyn Session + Send + '_> {
        match self {
            Self::Offline { asr, hotwords, cfg } => {
                Box::new(Committer::new(asr, hotwords.clone(), cfg.clone()))
            }
            Self::Streaming(s) => Box::new(s.session()),
        }
    }

    /// Runs a tiny decode so the first real dictation doesn't pay for lazy initialisation.
    pub fn warm_up(&self) -> Result<()> {
        let mut s = self.session();
        s.push(&vec![0.0; 16_000])?;
        s.finish().map(|_| ())
    }
}
