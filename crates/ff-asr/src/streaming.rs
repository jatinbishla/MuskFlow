//! Native streaming Parakeet (cache-aware "unified" models) through sherpa-onnx.
//!
//! Audio is encoded chunk by chunk as it arrives, so nothing is ever decoded
//! twice and key-up only has to flush the last chunk.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, anyhow};
use sherpa_onnx::{
    OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

use crate::{Hypothesis, Session, SessionStats, Word, join_words};

const SAMPLE_RATE: i32 = 16_000;
/// Silence appended at key-up so the encoder flushes its final chunk.
const TAIL_PADDING_SECS: f32 = 0.6;

#[derive(Debug, Clone)]
pub struct StreamingConfig {
    pub model_dir: PathBuf,
    pub num_threads: i32,
    pub provider: String,
}

impl StreamingConfig {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            num_threads: 2,
            provider: "cpu".into(),
        }
    }
}

pub struct StreamingParakeet {
    recognizer: OnlineRecognizer,
    name: String,
}

impl StreamingParakeet {
    pub fn load(cfg: &StreamingConfig) -> Result<Self> {
        let file = |name: &str| -> Result<String> {
            let p = cfg.model_dir.join(name);
            if !p.exists() {
                return Err(anyhow!("missing model file {}", p.display()));
            }
            Ok(p.to_string_lossy().into_owned())
        };
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(file("encoder.int8.onnx")?),
            decoder: Some(file("decoder.int8.onnx")?),
            joiner: Some(file("joiner.int8.onnx")?),
        };
        config.model_config.tokens = Some(file("tokens.txt")?);
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some(cfg.provider.clone());
        config.decoding_method = Some("greedy_search".into());
        config.enable_endpoint = false; // the hotkey decides when the utterance ends

        let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
            anyhow!(
                "could not load streaming model from {}",
                cfg.model_dir.display()
            )
        })?;
        let name = cfg
            .model_dir
            .file_name()
            .map(|n| {
                n.to_string_lossy()
                    .trim_start_matches("sherpa-onnx-nemo-")
                    .to_string()
            })
            .unwrap_or_else(|| "parakeet-streaming".into());
        Ok(Self { recognizer, name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn session(&self) -> StreamingSession<'_> {
        StreamingSession {
            recognizer: &self.recognizer,
            stream: self.recognizer.create_stream(),
            words: Vec::new(),
            final_count: 0,
            samples: 0,
            stats: SessionStats::default(),
        }
    }
}

pub struct StreamingSession<'a> {
    recognizer: &'a OnlineRecognizer,
    stream: OnlineStream,
    words: Vec<Word>,
    /// Words before this index are reported as final. The last word can still grow.
    final_count: usize,
    samples: usize,
    stats: SessionStats,
}

impl StreamingSession<'_> {
    fn decode_ready(&mut self) {
        let t = Instant::now();
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
            self.stats.decodes += 1;
        }
        self.stats.decode_time += t.elapsed();
        if let Some(r) = self.recognizer.get_result(&self.stream) {
            self.words = words_from_tokens(&r.tokens, r.timestamps.as_deref());
        }
    }
}

impl Session for StreamingSession<'_> {
    fn append(&mut self, pcm: &[f32]) {
        self.samples += pcm.len();
        self.stream.accept_waveform(SAMPLE_RATE, pcm);
    }

    fn push(&mut self, pcm: &[f32]) -> Result<Vec<Word>> {
        self.samples += pcm.len();
        self.stream.accept_waveform(SAMPLE_RATE, pcm);
        self.decode_ready();
        let stable = self.words.len().saturating_sub(1);
        if stable <= self.final_count {
            return Ok(Vec::new());
        }
        let newly = self.words[self.final_count..stable].to_vec();
        self.final_count = stable;
        Ok(newly)
    }

    fn partial_text(&self) -> (String, String) {
        let split = self.final_count.min(self.words.len());
        (
            join_words(&self.words[..split]),
            join_words(&self.words[split..]),
        )
    }

    fn audio_secs(&self) -> f32 {
        self.samples as f32 / SAMPLE_RATE as f32
    }

    fn finish(mut self: Box<Self>) -> Result<(Hypothesis, SessionStats)> {
        let t = Instant::now();
        let pad = vec![0.0; (TAIL_PADDING_SECS * SAMPLE_RATE as f32) as usize];
        self.stream.accept_waveform(SAMPLE_RATE, &pad);
        self.stream.input_finished();
        self.decode_ready();
        self.stats.tail_secs = TAIL_PADDING_SECS;
        self.stats.tail_time = t.elapsed();
        let stats = self.stats.clone();
        Ok((
            Hypothesis {
                words: std::mem::take(&mut self.words),
            },
            stats,
        ))
    }
}

/// Same grouping rules as the offline backend; online results carry start times only.
fn words_from_tokens(tokens: &[String], ts: Option<&[f32]>) -> Vec<Word> {
    crate::parakeet::tokens_to_words(tokens, ts, None, f32::MAX)
}
