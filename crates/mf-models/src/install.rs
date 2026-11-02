//! Downloading and unpacking model files.
//!
//! Downloads stream straight into the unpacker, so a 1 GB archive never sits on
//! disk twice. Everything lands in a `.partial-*` folder first and is renamed
//! into place only when complete, so an interrupted install never leaves a
//! half-written model that looks installed.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};

use crate::Tier;

const SHERPA_ASR: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// A single file saved as `name`.
    File,
    /// A `.tar.bz2` whose top-level folder is `name`.
    TarBz2,
}

/// One downloadable thing a tier needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// File or folder name inside the models directory.
    pub name: String,
    pub url: String,
    format: Format,
    pub approx_mb: u32,
}

impl Artifact {
    pub(crate) fn silero_vad() -> Artifact {
        Artifact {
            name: "silero_vad.onnx".into(),
            url: format!("{SHERPA_ASR}/silero_vad.onnx"),
            format: Format::File,
            approx_mb: 2,
        }
    }

    pub(crate) fn sherpa_asr(dir: &str, approx_mb: u32) -> Artifact {
        Artifact {
            name: dir.to_string(),
            url: format!("{SHERPA_ASR}/{dir}.tar.bz2"),
            format: Format::TarBz2,
            approx_mb,
        }
    }

    pub fn is_installed(&self, models: &Path) -> bool {
        match self.format {
            Format::File => models.join(&self.name).is_file(),
            Format::TarBz2 => models.join(&self.name).is_dir(),
        }
    }
}

/// Downloads every artifact the given tiers need that isn't already present.
pub fn install(models: &Path, tiers: &[Tier]) -> Result<()> {
    fs::create_dir_all(models).with_context(|| format!("could not create {}", models.display()))?;
    let mut todo: Vec<Artifact> = Vec::new();
    for tier in tiers {
        for a in tier.artifacts() {
            if !a.is_installed(models) && !todo.contains(&a) {
                todo.push(a);
            }
        }
    }
    if todo.is_empty() {
        eprintln!("· everything for {} is already installed", names(tiers));
        return Ok(());
    }
    let total: u32 = todo.iter().map(|a| a.approx_mb).sum();
    eprintln!(
        "· installing {} ({} files, ~{total} MB) into {}",
        names(tiers),
        todo.len(),
        models.display()
    );
    for a in &todo {
        fetch(models, a).with_context(|| format!("could not install {}", a.name))?;
    }
    eprintln!("· done");
    Ok(())
}

fn names(tiers: &[Tier]) -> String {
    tiers
        .iter()
        .map(|t| t.name())
        .collect::<Vec<_>>()
        .join(", ")
}

fn fetch(models: &Path, a: &Artifact) -> Result<()> {
    let mut resp = ureq::get(&a.url)
        .call()
        .with_context(|| format!("GET {}", a.url))?;
    let len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let bar = ProgressBar::new(len.unwrap_or(a.approx_mb as u64 * 1_000_000));
    bar.set_style(
        ProgressStyle::with_template(
            "  {msg:<48} {bar:30} {bytes:>9}/{total_bytes:<9} {bytes_per_sec:>11} {eta:>4}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("━╸ "),
    );
    bar.set_message(a.name.clone());
    let reader = bar.wrap_read(resp.body_mut().as_reader());

    let partial = models.join(format!(".partial-{}", a.name));
    if partial.exists() {
        remove(&partial)?;
    }
    match a.format {
        Format::File => {
            let mut out = fs::File::create(&partial)?;
            copy(reader, &mut out)?;
            fs::rename(&partial, models.join(&a.name))?;
        }
        Format::TarBz2 => {
            fs::create_dir_all(&partial)?;
            // `unpack` refuses entries that would escape the target folder.
            tar::Archive::new(bzip2::read::BzDecoder::new(reader)).unpack(&partial)?;
            let inner = partial.join(&a.name);
            if !inner.is_dir() {
                bail!("archive from {} has no top-level {}/ folder", a.url, a.name);
            }
            fs::rename(&inner, models.join(&a.name))?;
            fs::remove_dir_all(&partial)?;
        }
    }
    bar.finish();
    Ok(())
}

fn copy(mut r: impl Read, w: &mut impl io::Write) -> io::Result<u64> {
    io::copy(&mut r, w)
}

fn remove(p: &Path) -> io::Result<()> {
    if p.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}
