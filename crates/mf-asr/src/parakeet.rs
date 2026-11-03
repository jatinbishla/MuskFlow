//! NVIDIA Parakeet-TDT (NeMo transducer) through sherpa-onnx.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use crate::hotwords::Vocab;
use crate::{Hypothesis, Transcriber, Word};

const SAMPLE_RATE: i32 = 16_000;

#[derive(Debug, Clone)]
pub struct ParakeetConfig {
    /// Directory holding `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt`.
    pub model_dir: PathBuf,
    pub num_threads: i32,
    /// `"cpu"` or `"coreml"`.
    pub provider: String,
    /// Enables beam search so per-utterance hotwords can bias decoding. Greedy is faster.
    pub hotwords: bool,
    pub hotwords_score: f32,
}

impl ParakeetConfig {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            num_threads: 4,
            provider: "cpu".into(),
            hotwords: false,
            hotwords_score: 1.5,
        }
    }
}

pub struct ParakeetTranscriber {
    recognizer: OfflineRecognizer,
    name: String,
    /// Set when hotwords are enabled: spells them in the model's own pieces.
    vocab: Option<Vocab>,
}

impl ParakeetTranscriber {
    pub fn load(cfg: &ParakeetConfig) -> Result<Self> {
        let file = |name: &str| -> Result<String> {
            let p = cfg.model_dir.join(name);
            if !p.exists() {
                return Err(anyhow!(
                    "missing model file {} (run: muskflow models install)",
                    p.display()
                ));
            }
            Ok(p.to_string_lossy().into_owned())
        };

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(file("encoder.int8.onnx")?),
            decoder: Some(file("decoder.int8.onnx")?),
            joiner: Some(file("joiner.int8.onnx")?),
        };
        config.model_config.tokens = Some(file("tokens.txt")?);
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.provider = Some(cfg.provider.clone());
        if cfg.hotwords {
            config.decoding_method = Some("modified_beam_search".into());
            config.hotwords_score = cfg.hotwords_score;
        } else {
            config.decoding_method = Some("greedy_search".into());
        }

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow!("could not load Parakeet from {}", cfg.model_dir.display()))?;
        Ok(Self {
            recognizer,
            name: model_name(&cfg.model_dir),
            vocab: if cfg.hotwords {
                Some(Vocab::load(&cfg.model_dir.join("tokens.txt"))?)
            } else {
                None
            },
        })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn name(&self) -> &str {
        &self.name
    }

    fn transcribe(&self, pcm: &[f32], hotwords: &[String]) -> Result<Hypothesis> {
        if pcm.is_empty() {
            return Ok(Hypothesis::default());
        }
        let stream = match &self.vocab {
            Some(vocab) if !hotwords.is_empty() => self
                .recognizer
                .create_stream_with_hotwords(&vocab.hotword_lines(hotwords)),
            _ => self.recognizer.create_stream(),
        };
        stream.accept_waveform(SAMPLE_RATE, pcm);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("decoder returned no result"))?;
        let audio_end = pcm.len() as f32 / SAMPLE_RATE as f32;
        Ok(Hypothesis {
            words: tokens_to_words(
                &result.tokens,
                result.timestamps.as_deref(),
                result.durations.as_deref(),
                audio_end,
            ),
        })
    }
}

fn model_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| {
            n.to_string_lossy()
                .trim_start_matches("sherpa-onnx-nemo-")
                .to_string()
        })
        .unwrap_or_else(|| "parakeet".into())
}

/// Groups SentencePiece tokens into words. A token starting with a space (or `▁`)
/// begins a new word; a bare `" "` token (Parakeet emits these before digits)
/// means the *next* token begins one. A word ends where the next word begins, or
/// at its last token's start + duration when the model reports durations (TDT does).
pub(crate) fn tokens_to_words(
    tokens: &[String],
    ts: Option<&[f32]>,
    durs: Option<&[f32]>,
    audio_end: f32,
) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut boundary = true;
    for (i, tok) in tokens.iter().enumerate() {
        let start = ts.and_then(|t| t.get(i).copied()).unwrap_or(0.0);
        let tok_end = match (ts, durs) {
            (Some(t), Some(d)) if d.get(i).is_some_and(|d| *d > 0.0) => t[i] + d[i],
            (Some(t), _) => t.get(i + 1).copied().unwrap_or(audio_end),
            _ => audio_end,
        };
        let new_word = boundary || tok.starts_with(' ') || tok.starts_with('▁');
        let piece = tok.trim_start_matches([' ', '▁']);
        boundary = new_word && piece.is_empty();
        if boundary {
            continue;
        }
        if new_word {
            words.push(Word {
                text: piece.to_string(),
                start,
                end: tok_end,
            });
        } else if let Some(w) = words.last_mut() {
            w.text.push_str(piece);
            w.end = tok_end;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn groups_subword_tokens_into_words() {
        let toks = s(&[" Hel", "lo", " wor", "ld", "."]);
        let ts = [0.0, 0.2, 0.5, 0.7, 0.9];
        let words = tokens_to_words(&toks, Some(&ts), None, 1.2);
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["Hello", "world."]
        );
        assert_eq!(words[0].start, 0.0);
        assert_eq!(words[0].end, 0.5);
        assert_eq!(words[1].end, 1.2);
    }

    #[test]
    fn bare_space_token_starts_next_word() {
        let toks = s(&[" like", " ", "9", "0", "%", ","]);
        let ts = [0.0, 0.3, 0.4, 0.5, 0.6, 0.7];
        let words = tokens_to_words(&toks, Some(&ts), None, 1.0);
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["like", "90%,"]
        );
        assert_eq!(words[1].start, 0.4);
    }

    #[test]
    fn uses_tdt_durations_for_word_end() {
        let toks = s(&["▁hi", "▁there"]);
        let words = tokens_to_words(&toks, Some(&[0.0, 1.0]), Some(&[0.3, 0.4]), 2.0);
        assert_eq!(words[0].end, 0.3);
        assert_eq!(words[1].end, 1.4);
    }
}
