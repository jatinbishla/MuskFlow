//! `muskflow bench`: every clip through every tier, side by side.
//!
//! The product bar is that Lite is as accurate as Max, so the report leads with
//! the accuracy gap between tiers, not just each tier's own numbers. Two stages
//! are scored: the ASR against what was *said* (`<id>.txt`), and the final
//! cleaned text against what the user *meant* (`<id>.clean.txt`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use mf_audio::load_wav_16k;
use mf_cleanup::Cleaned;
use mf_eval::{Wer, wer};
use mf_models::{Tier, dir_size};

use crate::{Common, Replay, dictionary, llm_path, load_cleaner, load_engine, replay};

struct Clip {
    id: String,
    wav: PathBuf,
    /// What was said.
    reference: String,
    /// What the speaker meant, if known.
    clean: Option<String>,
    pcm: Vec<f32>,
}

struct ClipResult {
    asr: Replay,
    asr_wer: Wer,
    cleaned: Option<Cleaned>,
    final_wer: Option<Wer>,
    /// Final WER if we only ran the deterministic rules (no LLM): the bar the LLM must beat.
    rules_wer: Option<Wer>,
}

impl ClipResult {
    /// Key-up → final text: ASR finish plus cleanup.
    fn total(&self) -> Duration {
        self.asr.latency + self.cleaned.as_ref().map_or(Duration::ZERO, |c| c.time)
    }
}

struct TierRun {
    tier: Tier,
    load: Duration,
    size_mb: f64,
    /// Cleanup LLM used, if any.
    llm: Option<String>,
    results: Vec<ClipResult>,
}

pub fn run(
    c: &Common,
    tiers: &[String],
    clips_dir: &Path,
    fast: bool,
    show: bool,
    no_cleanup: bool,
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

    let dict = dictionary(c)?;
    let mut runs = Vec::new();
    for tier in tiers {
        if !tier.asr_path(&c.models).is_dir() {
            eprintln!("· {tier}: not installed, skipping (muskflow models install {tier})");
            continue;
        }
        let t = Instant::now();
        let engine = load_engine(c, &tier.asr_path(&c.models))?;
        let cleaner = if no_cleanup || !llm_path(c, tier).is_file() {
            None
        } else {
            Some(load_cleaner(c, &llm_path(c, tier))?)
        };
        let load = t.elapsed();

        let mut results = Vec::new();
        for clip in &clips {
            let asr = replay(&engine, &clip.pcm, fast, false)?;
            let asr_wer = wer(&clip.reference, &asr.text);
            let cleaned = match &cleaner {
                Some(cl) => Some(cl.clean(&asr.text)?),
                None => None,
            };
            let final_wer = match (&cleaned, &clip.clean) {
                (Some(cd), Some(reference)) => Some(wer(reference, &cd.text)),
                _ => None,
            };
            let rules_wer = clip
                .clean
                .as_ref()
                .map(|reference| wer(reference, &mf_cleanup::rules::clean(&dict.apply(&asr.text))));
            let r = ClipResult {
                asr,
                asr_wer,
                cleaned,
                final_wer,
                rules_wer,
            };
            eprintln!(
                "  {tier:<8} {:<16} ASR WER {:>5.1}%  final WER {:>6}  key-up → text {:>5.0} ms",
                clip.id,
                100.0 * r.asr_wer.rate(),
                r.final_wer
                    .map_or("—".into(), |w| format!("{:.1}%", 100.0 * w.rate())),
                r.total().as_secs_f64() * 1e3
            );
            results.push(r);
        }
        let mut size = dir_size(&tier.asr_path(&c.models));
        if cleaner.is_some() {
            size += std::fs::metadata(llm_path(c, tier)).map_or(0, |m| m.len());
        }
        runs.push(TierRun {
            tier,
            load,
            size_mb: size as f64 / 1e6,
            llm: cleaner.as_ref().map(|c| c.llm().name().to_string()),
            results,
        });
    }
    if runs.is_empty() {
        bail!("no tiers installed; run: muskflow models install all");
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
        let clean = std::fs::read_to_string(wav.with_extension("clean.txt"))
            .ok()
            .map(|s| s.trim().to_string());
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
            clean,
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

fn sum(ws: impl Iterator<Item = Wer>) -> Wer {
    ws.fold(Wer::default(), |a, b| a + b)
}

fn gap(rate: f32, best: f32) -> String {
    if rate <= best {
        "—".into()
    } else {
        format!("+{:.1} pts", 100.0 * (rate - best))
    }
}

fn render(clips: &[Clip], runs: &[TierRun], fast: bool, show: bool) -> String {
    let mut s = String::new();
    let asr_rate = |r: &TierRun| sum(r.results.iter().map(|x| x.asr_wer)).rate();
    let final_rate = |r: &TierRun| {
        let ws: Vec<Wer> = r.results.iter().filter_map(|x| x.final_wer).collect();
        (!ws.is_empty()).then(|| sum(ws.into_iter()).rate())
    };
    let best_asr = runs.iter().map(asr_rate).fold(f32::INFINITY, f32::min);
    let best_final = runs
        .iter()
        .filter_map(final_rate)
        .fold(f32::INFINITY, f32::min);
    let rules_rate = |r: &TierRun| {
        let ws: Vec<Wer> = r.results.iter().filter_map(|x| x.rules_wer).collect();
        (!ws.is_empty()).then(|| sum(ws.into_iter()).rate())
    };

    let _ = writeln!(
        s,
        "## Tier comparison{}\n",
        if fast { " (fast replay)" } else { "" }
    );
    let _ = writeln!(
        s,
        "| Tier | Models | Size | Load | **Final WER** (vs meant) | Gap | Rules-only WER | ASR WER (vs said) | Gap | Key-up → text p50 / max | Cleanup p50 | Guard fallbacks | CPU while speaking |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in runs {
        let fin = final_rate(r);
        let total: Vec<f64> = r
            .results
            .iter()
            .map(|x| x.total().as_secs_f64() * 1e3)
            .collect();
        let clean_ms: Vec<f64> = r
            .results
            .iter()
            .filter_map(|x| x.cleaned.as_ref())
            .map(|c| c.time.as_secs_f64() * 1e3)
            .collect();
        let fallbacks = r
            .results
            .iter()
            .filter(|x| {
                x.cleaned
                    .as_ref()
                    .is_some_and(|c| c.path == mf_cleanup::Path::Fallback)
            })
            .count();
        let audio: f32 = r.results.iter().map(|x| x.asr.audio_secs).sum();
        let decode: f32 = r
            .results
            .iter()
            .map(|x| x.asr.stats.decode_time.as_secs_f32())
            .sum();
        let _ = writeln!(
            s,
            "| **{}** | {}{} | {:.0} MB | {:.1} s | **{}** | {} | {} | {} | {} | {:.0} / {:.0} ms | {} | {} | {:.0}% |",
            r.tier,
            r.tier.asr_label(),
            r.llm
                .as_ref()
                .map_or(String::new(), |name| format!(" + {name}")),
            r.size_mb,
            r.load.as_secs_f32(),
            fin.map_or("—".into(), pct),
            fin.map_or("—".into(), |f| gap(f, best_final)),
            rules_rate(r).map_or("—".into(), pct),
            pct(asr_rate(r)),
            gap(asr_rate(r), best_asr),
            percentile(total.clone(), 0.5),
            percentile(total, 1.0),
            if clean_ms.is_empty() {
                "—".into()
            } else {
                format!("{:.0} ms", percentile(clean_ms, 0.5))
            },
            if r.llm.is_some() {
                format!("{fallbacks}/{}", r.results.len())
            } else {
                "—".into()
            },
            100.0 * decode / audio
        );
    }

    let _ = writeln!(s, "\n### Per clip: final WER (ASR WER)\n");
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
            let x = &r.results[i];
            let fin = x.final_wer.map_or("—".into(), |w| pct(w.rate()));
            let _ = write!(s, " {fin} ({}) |", pct(x.asr_wer.rate()));
        }
        let _ = writeln!(s);
    }

    if show {
        let _ = writeln!(s, "\n### Transcripts\n");
        for (i, clip) in clips.iter().enumerate() {
            let _ = writeln!(s, "**{}** ({})\n", clip.id, clip.wav.display());
            let _ = writeln!(s, "- said: {}", clip.reference);
            if let Some(c) = &clip.clean {
                let _ = writeln!(s, "- meant: {c}");
            }
            for r in runs {
                let x = &r.results[i];
                let _ = writeln!(s, "- {} ASR: {}", r.tier, x.asr.text);
                if let Some(cd) = &x.cleaned {
                    let tag = match cd.path {
                        mf_cleanup::Path::Rules => "rules".to_string(),
                        mf_cleanup::Path::Llm => "llm".to_string(),
                        mf_cleanup::Path::Fallback => format!(
                            "fallback: {}; llm said \"{}\"",
                            cd.verdict.as_ref().and_then(|v| v.reason).unwrap_or("?"),
                            cd.llm_text.as_deref().unwrap_or("")
                        ),
                    };
                    let _ = writeln!(s, "- {} final ({tag}): {}", r.tier, cd.text);
                }
            }
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(
        s,
        "\nWER ignores case, punctuation, number formatting and fillers. Final WER also counts uncorrected self-corrections (\"Thursday, no wait, Friday\" should become \"Friday\") and anything the LLM added."
    );
    s
}
