//! Speech recognition for MuskFlow.
//!
//! [`Transcriber`] is the backend-agnostic interface; [`Committer`] turns any
//! offline transcriber into a streaming one by locking in words as soon as two
//! consecutive decodes agree on them (LocalAgreement-2), so that on key-up only
//! the last few seconds of audio are left to decode.

mod committer;
mod hotwords;
mod parakeet;
mod phrases;
mod streaming;
mod whisper;

pub use committer::{Committer, CommitterConfig};
pub use hotwords::Vocab;
pub use parakeet::{ParakeetConfig, ParakeetTranscriber};
pub use phrases::PhraseSession;
pub use streaming::{StreamingConfig, StreamingParakeet};
pub use whisper::{WhisperConfig, WhisperTranscriber};

use std::path::{Path, PathBuf};
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
        asr: Box<dyn Transcriber>,
        hotwords: Vec<String>,
        cfg: CommitterConfig,
        /// Decode VAD-cut phrases instead of re-decoding a sliding window. Needed by
        /// models without reliable word timestamps (Whisper).
        by_phrase: bool,
        vad_model: Option<PathBuf>,
        vad: Option<mf_audio::SpeechTrimmer>,
    },
    Streaming(StreamingParakeet),
}

/// Longest phrase decoded in one go when the speaker never pauses.
const MAX_PHRASE_SECS: f32 = 8.0;

impl Engine {
    /// Loads the model in `model_dir`. The backend follows the directory name:
    /// "streaming" → native streaming Parakeet, "whisper" → Whisper by phrase,
    /// anything else → Parakeet with the LocalAgreement committer.
    pub fn load(
        model_dir: &Path,
        num_threads: i32,
        provider: &str,
        hotwords: Vec<String>,
        hotwords_score: f32,
    ) -> Result<Self> {
        let dir = model_dir.to_string_lossy();
        if dir.contains("streaming") {
            let mut cfg = StreamingConfig::new(model_dir);
            cfg.num_threads = num_threads;
            cfg.provider = provider.into();
            return Ok(Self::Streaming(StreamingParakeet::load(&cfg)?));
        }
        let by_phrase = dir.contains("whisper");
        let asr: Box<dyn Transcriber> = if by_phrase {
            let mut cfg = WhisperConfig::new(model_dir);
            cfg.num_threads = num_threads;
            cfg.provider = provider.into();
            Box::new(WhisperTranscriber::load(&cfg)?)
        } else {
            let mut cfg = ParakeetConfig::new(model_dir);
            cfg.num_threads = num_threads;
            cfg.provider = provider.into();
            cfg.hotwords = !hotwords.is_empty();
            cfg.hotwords_score = hotwords_score;
            Box::new(ParakeetTranscriber::load(&cfg)?)
        };
        Ok(Self::Offline {
            asr,
            hotwords,
            cfg: CommitterConfig::default(),
            by_phrase,
            vad_model: None,
            vad: None,
        })
    }

    /// Gives offline sessions Silero VAD: to tell silence from dropped speech, and to cut
    /// phrases for by-phrase models. Required for Whisper. No-op for streaming models.
    pub fn with_vad(mut self, model: &Path) -> Result<Self> {
        if let Self::Offline { vad, vad_model, .. } = &mut self {
            *vad = Some(mf_audio::SpeechTrimmer::load(
                model,
                mf_audio::VadConfig::default(),
            )?);
            *vad_model = Some(model.to_path_buf());
        }
        Ok(self)
    }

    /// Tunes how often an offline model re-decodes while the user speaks. No-op for streaming models.
    pub fn with_committer(mut self, new: CommitterConfig) -> Self {
        if let Self::Offline { cfg, .. } = &mut self {
            *cfg = new;
        }
        self
    }

    /// The underlying one-shot transcriber, for decoding a whole file at once.
    /// `None` for native streaming models.
    pub fn transcriber(&self) -> Option<&dyn Transcriber> {
        match self {
            Self::Offline { asr, .. } => Some(asr.as_ref()),
            Self::Streaming(_) => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Offline { asr, .. } => asr.name(),
            Self::Streaming(s) => s.name(),
        }
    }

    pub fn session(&self) -> Result<Box<dyn Session + Send + '_>> {
        Ok(match self {
            Self::Offline {
                asr,
                hotwords,
                by_phrase: true,
                vad_model,
                ..
            } => {
                let model = vad_model.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("{} needs VAD: call Engine::with_vad", asr.name())
                })?;
                let segmenter = mf_audio::SpeechSegmenter::load(
                    model,
                    mf_audio::VadConfig::default(),
                    MAX_PHRASE_SECS,
                )?;
                Box::new(PhraseSession::new(
                    asr.as_ref(),
                    hotwords.clone(),
                    segmenter,
                ))
            }
            Self::Offline {
                asr,
                hotwords,
                cfg,
                vad,
                ..
            } => {
                let c = Committer::new(asr.as_ref(), hotwords.clone(), cfg.clone());
                Box::new(match vad {
                    Some(v) => c.with_vad(v),
                    None => c,
                })
            }
            Self::Streaming(s) => Box::new(s.session()),
        })
    }

    /// Runs a tiny decode so the first real dictation doesn't pay for lazy initialisation.
    pub fn warm_up(&self) -> Result<()> {
        match self {
            Self::Offline { asr, .. } => asr.transcribe(&vec![0.0; 16_000], &[]).map(|_| ()),
            Self::Streaming(_) => {
                let mut s = self.session()?;
                s.push(&vec![0.0; 16_000])?;
                s.finish().map(|_| ())
            }
        }
    }
}
