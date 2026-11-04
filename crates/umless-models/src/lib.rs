//! Umless model tiers, and installing them.
//!
//! Every tier runs the same pipeline; tiers differ only in model size. The bar
//! is that Lite must be as accurate as Max, so every benchmark runs all three.
//!
//! [`install`] downloads everything a tier needs, so a user never has to visit
//! another repository: `umless models install lite|standard|max|all`.

mod install;

pub use install::{Artifact, install};

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Any laptop, CPU only, ~8 GB RAM.
    Lite,
    /// Apple Silicon or any laptop with a decent CPU. The default.
    Standard,
    /// 16 GB+ Apple Silicon or a discrete GPU. Multilingual.
    Max,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Lite, Tier::Standard, Tier::Max];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Lite => "lite",
            Tier::Standard => "standard",
            Tier::Max => "max",
        }
    }

    /// Speech recognition model directory (inside the models folder).
    pub fn asr_dir(self) -> &'static str {
        match self {
            Tier::Lite => "sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000-int8",
            Tier::Standard => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
            Tier::Max => "sherpa-onnx-whisper-turbo",
        }
    }

    pub fn asr_label(self) -> &'static str {
        match self {
            Tier::Lite => "Parakeet-TDT 110M int8 (English)",
            Tier::Standard => "Parakeet-TDT 0.6B v3 int8 (25 European languages)",
            Tier::Max => "Whisper large-v3-turbo (99 languages)",
        }
    }

    pub fn asr_path(self, models: &Path) -> PathBuf {
        models.join(self.asr_dir())
    }

    /// Approximate ASR download, in MB.
    fn asr_mb(self) -> u32 {
        match self {
            Tier::Lite => 108,
            Tier::Standard => 487,
            Tier::Max => 564,
        }
    }

    /// Cleanup LLM: (Hugging Face repo, GGUF file, approx MB).
    fn llm(self) -> (&'static str, &'static str, u32) {
        match self {
            Tier::Lite => ("unsloth/Qwen3.5-0.8B-GGUF", "Qwen3.5-0.8B-Q4_K_M.gguf", 533),
            Tier::Standard => ("unsloth/Qwen3.5-2B-GGUF", "Qwen3.5-2B-Q4_K_M.gguf", 1281),
            Tier::Max => (
                "unsloth/Qwen3-4B-Instruct-2507-GGUF",
                "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
                2497,
            ),
        }
    }

    pub fn llm_label(self) -> &'static str {
        match self {
            Tier::Lite => "Qwen3.5 0.8B Q4_K_M",
            Tier::Standard => "Qwen3.5 2B Q4_K_M",
            Tier::Max => "Qwen3 4B Instruct 2507 Q4_K_M",
        }
    }

    pub fn llm_path(self, models: &Path) -> PathBuf {
        models.join(self.llm().1)
    }

    /// Everything this tier needs on disk.
    pub fn artifacts(self) -> Vec<Artifact> {
        vec![
            Artifact::silero_vad(),
            Artifact::sherpa_asr(self.asr_dir(), self.asr_mb()),
            Artifact::hf_file(self.llm().0, self.llm().1, self.llm().2),
        ]
    }

    /// Approximate download size of this tier's models, in MB.
    pub fn download_mb(self) -> u32 {
        self.asr_mb() + self.llm().2 + 2
    }

    /// Whether this tier's models are downloaded.
    pub fn is_installed(self, models: &Path) -> bool {
        self.artifacts().iter().all(|a| a.is_installed(models))
    }

    /// Suggested tier for a machine with this much RAM. Never Max: it needs a fast
    /// GPU/ANE to feel instant, so users opt into it explicitly.
    pub fn recommended(ram_gb: f64) -> Tier {
        if ram_gb >= 8.0 {
            Tier::Standard
        } else {
            Tier::Lite
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Parses tier names, where `all` means every tier.
pub fn parse_tiers(names: &[String]) -> Result<Vec<Tier>> {
    let mut out = Vec::new();
    for n in names {
        if n.eq_ignore_ascii_case("all") {
            out.extend(Tier::ALL);
        } else {
            out.push(n.parse()?);
        }
    }
    out.dedup();
    Ok(out)
}

impl FromStr for Tier {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Tier::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| anyhow!("unknown tier {s:?} (expected lite, standard or max)"))
    }
}

/// Total size of a model directory on disk, in bytes.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m)
                if e.file_name().to_string_lossy().ends_with(".onnx")
                    || e.file_name() == "tokens.txt" =>
            {
                m.len()
            }
            _ => 0,
        })
        .sum()
}
