//! `muskflow bench`: every clip through every tier, side by side.
//!
//! The product bar is that Lite is as accurate as Max, so the report leads with
//! the accuracy gap between tiers, not just each tier's own numbers.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use mf_audio::load_wav_16k;
use mf_eval::{Wer, wer};
use mf_models::{Tier, dir_size};

use crate::{Common, Replay, load_engine, replay};

struct Clip {
    id: String,
    wav: PathBuf,
    reference: String,
    pcm: Vec<f32>,
}

struct TierRun {
    tier: Tier,
    load: Duration,
    size_mb: f64,
    /// Per clip, same order as the clip list.
    results: Vec<(Replay, Wer)>,
}

pub fn run(
    c: &Common,
    tiers: &[String],
    clips_dir: &Path,
    fast: bool,
    show: bool,
    out: Option<&Path>,
) -> Result<()> {
    let tiers: Vec<Tier> = tiers.iter().map(|t| t.parse()).collect::<Result<_>>()?;
    let clips = load_clips(clips_dir)?;
    let audio: f32 = clips.iter().map(|k| k.pcm.len() as f32 / 16_000.0).sum();
    eprintln!(
        "· {} clips, {:.0}s of audio, tiers: {} ({})",
        clips.len(),
        audio,
        tiers
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(", "),
        if fast {
            "fast replay: latency/CPU not realistic"
        } else {
            "real-time replay"
        }
    );

    let mut runs = Vec::new();
    for tier in tiers {
        if !tier.is_installed(&c.models) {
            eprintln!("· {tier}: not installed, skipping (scripts/fetch-models.sh {tier})");
            continue;
        }
        let t = Instant::now();
        let engine = load_engine(c, &tier.asr_path(&c.models))?;
        let load = t.elapsed();
        let mut results = Vec::new();
        for clip in &clips {
            let r = replay(&engine, &clip.pcm, fast, false)?;
            let w = wer(&clip.reference, &r.text);
            eprintln!(
                "  {tier:<8} {:<12} WER {:>5.1}%  key-up → text {:>5.0} ms",
                clip.id,
                100.0 * w.rate(),
                r.latency.as_secs_f64() * 1e3
            );
            results.push((r, w));
        }
        runs.push(TierRun {
            tier,
            load,
            size_mb: dir_size(&tier.asr_path(&c.models)) as f64 / 1e6,
            results,
        });
    }
    if runs.is_empty() {
        bail!("no tiers installed; run scripts/fetch-models.sh lite standard max");
    }

    let report = render(&clips, &runs, fast, show);
    println!("{report}");
    if let Some(path) = out {
        std::fs::write(path, &report)
            .with_context(|| format!("could not write {}", path.display()))?;
        eprintln!("· wrote {}", path.display());
    }
    Ok(())
}

fn load_clips(dir: &Path) -> Result<Vec<Clip>> {
    let mut clips = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| {
        format!(
            "no clips in {} (run scripts/make-bench-clips.sh)",
            dir.display()
        )
    })?;
    for e in entries.flatten() {
        let wav = e.path();
        if wav.extension().is_none_or(|x| x != "wav") {
            continue;
        }
        let txt = wav.with_extension("txt");
        let Ok(reference) = std::fs::read_to_string(&txt) else {
            eprintln!("· skipping {} (no {})", wav.display(), txt.display());
            continue;
        };
        let id = wav
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        clips.push(Clip {
            id,
            pcm: load_wav_16k(&wav)?,
            wav,
            reference: reference.trim().to_string(),
        });
    }
    clips.sort_by(|a, b| a.id.cmp(&b.id));
    if clips.is_empty() {
        bail!("no <id>.wav + <id>.txt pairs in {}", dir.display());
    }
    Ok(clips)
}

fn pct(x: f32) -> String {
    format!("{:.1}%", 100.0 * x)
}

fn percentile(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn render(clips: &[Clip], runs: &[TierRun], fast: bool, show: bool) -> String {
    let mut s = String::new();
    let best = runs
        .iter()
        .map(|r| {
            r.results
                .iter()
                .map(|(_, w)| *w)
                .fold(Wer::default(), |a, b| a + b)
                .rate()
        })
        .fold(f32::INFINITY, f32::min);

    let _ = writeln!(
        s,
        "## Tier comparison{}\n",
        if fast { " (fast replay)" } else { "" }
    );
    let _ = writeln!(
        s,
        "| Tier | ASR model | Size | Load | WER (all clips) | Gap to best | Worst clip | Key-up → text p50 / max | CPU while speaking |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|");
    for r in runs {
        let total = r
            .results
            .iter()
            .map(|(_, w)| *w)
            .fold(Wer::default(), |a, b| a + b);
        let (worst_i, worst) = r
            .results
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.1.rate().total_cmp(&b.1.1.rate()))
            .map(|(i, (_, w))| (i, w.rate()))
            .unwrap_or((0, 0.0));
        let lat: Vec<f64> = r
            .results
            .iter()
            .map(|(x, _)| x.latency.as_secs_f64() * 1e3)
            .collect();
        let audio: f32 = r.results.iter().map(|(x, _)| x.audio_secs).sum();
        let decode: f32 = r
            .results
            .iter()
            .map(|(x, _)| x.stats.decode_time.as_secs_f32())
            .sum();
        let _ = writeln!(
            s,
            "| **{}** | {} | {:.0} MB | {:.1} s | **{}** | {} | {} ({}) | {:.0} / {:.0} ms | {:.0}% |",
            r.tier,
            r.tier.asr_label(),
            r.size_mb,
            r.load.as_secs_f32(),
            pct(total.rate()),
            if total.rate() <= best {
                "—".to_string()
            } else {
                format!("+{:.1} pts", 100.0 * (total.rate() - best))
            },
            pct(worst),
            clips[worst_i].id,
            percentile(lat.clone(), 0.5),
            percentile(lat, 1.0),
            100.0 * decode / audio
        );
    }

    let _ = writeln!(s, "\n### WER per clip\n");
    let _ = write!(s, "| Clip |");
    for r in runs {
        let _ = write!(s, " {} |", r.tier);
    }
    let _ = write!(s, "\n|---|");
    for _ in runs {
        let _ = write!(s, "---|");
    }
    let _ = writeln!(s);
    for (i, clip) in clips.iter().enumerate() {
        let _ = write!(s, "| {} |", clip.id);
        for r in runs {
            let _ = write!(s, " {} |", pct(r.results[i].1.rate()));
        }
        let _ = writeln!(s);
    }

    if show {
        let _ = writeln!(s, "\n### Transcripts\n");
        for (i, clip) in clips.iter().enumerate() {
            let _ = writeln!(
                s,
                "**{}** ({})\n\n- reference: {}",
                clip.id,
                clip.wav.display(),
                clip.reference
            );
            for r in runs {
                let _ = writeln!(s, "- {}: {}", r.tier, r.results[i].0.text);
            }
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(
        s,
        "\nWER ignores case, punctuation, number formatting and fillers (uh/um): fillers are the cleanup LLM's job."
    );
    s
}
