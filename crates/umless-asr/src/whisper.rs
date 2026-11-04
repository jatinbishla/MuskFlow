//! OpenAI Whisper (large-v3-turbo etc.) through sherpa-onnx. Multilingual; used by the Max tier.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineWhisperModelConfig};

use crate::{Hypothesis, Transcriber};

const SAMPLE_RATE: i32 = 16_000;

#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Directory holding `<name>-encoder[.int8].onnx`, `<name>-decoder[.int8].onnx`, `<name>-tokens.txt`.
    pub model_dir: PathBuf,
    pub num_threads: i32,
    pub provider: String,
    /// ISO code such as `"en"` or `"hi"`; empty = auto-detect.
    pub language: String,
}

impl WhisperConfig {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            num_threads: 4,
            provider: "cpu".into(),
            language: String::new(),
        }
    }
}

pub struct WhisperTranscriber {
    recognizer: OfflineRecognizer,
    name: String,
}

impl WhisperTranscriber {
    pub fn load(cfg: &WhisperConfig) -> Result<Self> {
        let pick = |suffix: &str| -> Result<String> {
            find_file(&cfg.model_dir, &format!("{suffix}.int8.onnx"))
                .or_else(|| find_file(&cfg.model_dir, &format!("{suffix}.onnx")))
                .ok_or_else(|| anyhow!("no *{suffix}.onnx in {}", cfg.model_dir.display()))
        };
        let tokens = find_file(&cfg.model_dir, "tokens.txt")
            .ok_or_else(|| anyhow!("no *tokens.txt in {}", cfg.model_dir.display()))?;

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.whisper = OfflineWhisperModelConfig {
            encoder: Some(pick("encoder")?),
            decoder: Some(pick("decoder")?),
            language: (!cfg.language.is_empty()).then(|| cfg.language.clone()),
            task: Some("transcribe".into()),
            tail_paddings: -1,
            enable_token_timestamps: true,
            enable_segment_timestamps: false,
        };
        config.model_config.tokens = Some(tokens);
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some(cfg.provider.clone());
        config.decoding_method = Some("greedy_search".into());

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("could not load Whisper from {}", cfg.model_dir.display()))?;
        let name = cfg
            .model_dir
            .file_name()
            .map(|n| {
                n.to_string_lossy()
                    .trim_start_matches("sherpa-onnx-")
                    .to_string()
            })
            .unwrap_or_else(|| "whisper".into());
        Ok(Self { recognizer, name })
    }
}

impl Transcriber for WhisperTranscriber {
    fn name(&self) -> &str {
        &self.name
    }

    fn transcribe(&self, pcm: &[f32], _hotwords: &[String]) -> Result<Hypothesis> {
        if pcm.is_empty() {
            return Ok(Hypothesis::default());
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE, pcm);
        self.recognizer.decode(&stream);
        let r = stream
            .get_result()
            .ok_or_else(|| anyhow!("decoder returned no result"))?;
        let audio_end = pcm.len() as f32 / SAMPLE_RATE as f32;
        Ok(Hypothesis {
            words: crate::parakeet::tokens_to_words(
                &r.tokens,
                r.timestamps.as_deref(),
                None,
                audio_end,
            ),
        })
    }
}

fn find_file(dir: &Path, suffix: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
        })
        .map(|p| p.to_string_lossy().into_owned())
}
