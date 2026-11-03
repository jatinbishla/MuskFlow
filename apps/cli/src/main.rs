//! `muskflow` — headless MuskFlow for development, benchmarking and scripting.

mod app;
mod bench;
mod history;
mod hub;
mod settings;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use mf_asr::{CommitterConfig, Engine, Hypothesis, SessionStats, Word};
use mf_audio::{
    MicCapture, SpeechTrimmer, VadConfig, list_inputs, load_wav_16k, samples_to_secs, save_wav_16k,
};
use mf_cleanup::{Cleaner, Dictionary};
use mf_llm::{Llm, LlmConfig};
use mf_models::{Tier, parse_tiers};

#[derive(Parser)]
#[command(
    name = "muskflow",
    version,
    about = "Local voice dictation that writes what you meant"
)]
struct Cli {
    #[command(flatten)]
    common: Common,
    /// Defaults to `app` inside MuskFlow.app (launched with no arguments).
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Args, Clone)]
pub(crate) struct Common {
    /// Directory for downloaded models (`muskflow models install` fills it).
    #[arg(long, env = "MUSKFLOW_MODELS", default_value = "models", global = true)]
    pub(crate) models: PathBuf,
    /// Model tier: lite, standard or max.
    #[arg(long, env = "MUSKFLOW_TIER", default_value = "standard", global = true)]
    tier: Tier,
    /// Use this ASR model directory (inside --models) instead of the tier's.
    #[arg(long, global = true)]
    asr: Option<String>,
    /// Use this cleanup LLM (a .gguf file inside --models) instead of the tier's.
    #[arg(long, global = true)]
    pub(crate) llm: Option<String>,
    /// LLM layers to run on the GPU (Metal/CUDA/Vulkan); 0 = CPU only.
    #[arg(long, default_value_t = 999, global = true)]
    gpu_layers: u32,
    /// Inference threads; 0 = all cores, capped at 8.
    #[arg(long, default_value_t = 0, global = true)]
    threads: i32,
    /// Inference provider: cpu or coreml.
    #[arg(long, default_value = "cpu", global = true)]
    provider: String,
    /// Comma-separated names and terms to spell your way (added to --dictionary).
    #[arg(long, value_delimiter = ',', global = true)]
    hotwords: Vec<String>,
    /// Your dictionary: one name or term per line, spelled the way you want it.
    #[arg(long, env = "MUSKFLOW_DICTIONARY", global = true)]
    dictionary: Option<PathBuf>,
    /// Also bias the speech model towards dictionary terms, this much per token
    /// (Parakeet tiers; beam search). 0 = off. Above ~2 it starts inventing them.
    #[arg(long, default_value_t = 0.0, global = true)]
    hotword_boost: f32,
    /// Seconds of new audio between re-decodes while speaking (offline models).
    #[arg(long, default_value_t = 1.0, global = true)]
    step: f32,
    /// Seconds of already-committed audio re-fed as left context (offline models).
    #[arg(long, default_value_t = 1.5, global = true)]
    context: f32,
}

#[derive(Subcommand)]
enum Cmd {
    /// Transcribe WAV files in one pass. With several files, prints `<file>\t<text>` per line.
    Transcribe {
        #[arg(required = true)]
        wavs: Vec<PathBuf>,
        /// Skip VAD silence trimming.
        #[arg(long)]
        no_vad: bool,
    },
    /// Replay a WAV file as if it were live speech and measure key-up → text latency.
    Stream {
        wav: PathBuf,
        /// Feed audio as fast as possible instead of in real time.
        #[arg(long)]
        fast: bool,
    },
    /// Dictate from the microphone; press Enter to stop.
    Listen {
        /// Input device: part of its name or id (see `muskflow devices`). Default: system input.
        #[arg(long)]
        mic: Option<String>,
        /// Also save the recording as a 16 kHz WAV (replay it later with `stream`).
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Run in the background: hold Fn anywhere to dictate, and the cleaned text is typed at
    /// your cursor. Fn + Control is command mode. macOS; needs Input Monitoring and Accessibility.
    App {
        /// Input device: part of its name or id (see `muskflow devices`). Default: system input.
        #[arg(long)]
        mic: Option<String>,
    },
    /// List microphones.
    Devices,
    /// Download or list models. Everything MuskFlow needs comes through here.
    Models {
        #[command(subcommand)]
        action: ModelsCmd,
    },
    /// Run every clip through every tier and compare accuracy, latency, CPU and size.
    Bench {
        /// Tiers to run (comma-separated).
        #[arg(long, value_delimiter = ',', default_value = "lite,standard,max")]
        tiers: Vec<String>,
        /// Folder of <id>.wav + <id>.txt (reference) pairs; see scripts/make-bench-clips.sh.
        #[arg(long, default_value = "bench/clips")]
        clips: PathBuf,
        /// Feed audio as fast as possible: accurate WER, but latency and CPU numbers are not realistic.
        #[arg(long)]
        fast: bool,
        /// Print every hypothesis next to its reference.
        #[arg(long)]
        show: bool,
        /// Score the ASR only, without the cleanup LLM.
        #[arg(long)]
        no_cleanup: bool,
        /// Also write the report (markdown) to this file.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Clean up a piece of dictated text with the tier's LLM (reads stdin if no text given).
    Clean {
        text: Option<String>,
        /// clean, formal, casual or prompt.
        #[arg(long, default_value = "clean")]
        style: mf_cleanup::Style,
    },
    /// Print the cleanup prompt for `text` as prefix, a NUL byte, then suffix (for fine-tuning).
    #[command(hide = true)]
    Prompt {
        text: String,
        /// The compact prompt MuskFlow's fine-tuned models use.
        #[arg(long)]
        compact: bool,
        /// clean, formal, casual or prompt.
        #[arg(long, default_value = "clean")]
        style: mf_cleanup::Style,
    },
}

#[derive(Subcommand)]
enum ModelsCmd {
    /// Download the models for one or more tiers: lite, standard, max, or all.
    Install {
        #[arg(default_value = "standard", num_args = 1..)]
        tiers: Vec<String>,
    },
    /// Show which tiers are installed.
    List,
}

fn main() -> Result<()> {
    if app::in_app_bundle() {
        app::log_to_file();
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let cmd = match &cli.cmd {
        Some(cmd) => cmd,
        // Double-clicked MuskFlow.app: macOS starts the binary with no arguments.
        None if app::in_app_bundle() => &Cmd::App { mic: None },
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            return Ok(());
        }
    };
    match cmd {
        Cmd::Transcribe { wavs, no_vad } => transcribe(&cli.common, wavs, *no_vad),
        Cmd::Stream { wav, fast } => stream(&cli.common, wav, *fast),
        Cmd::Listen { mic, save } => listen(&cli.common, mic.as_deref(), save.as_deref()),
        Cmd::App { mic } => app::run(&cli.common, mic.as_deref()),
        Cmd::Devices => devices(),
        Cmd::Models {
            action: ModelsCmd::Install { tiers },
        } => mf_models::install(&cli.common.models, &parse_tiers(tiers)?),
        Cmd::Models {
            action: ModelsCmd::List,
        } => list_models(&cli.common.models),
        Cmd::Bench {
            tiers,
            clips,
            fast,
            show,
            no_cleanup,
            out,
        } => bench::run(
            &cli.common,
            tiers,
            clips,
            *fast,
            *show,
            *no_cleanup,
            out.as_deref(),
        ),
        Cmd::Clean { text, style } => clean(&cli.common, text.as_deref(), *style),
        Cmd::Prompt {
            text,
            compact,
            style,
        } => {
            let (prefix, suffix) = if *compact {
                mf_cleanup::build_compact_prompt(text, *style)
            } else {
                mf_cleanup::build_prompt(text)
            };
            print!("{prefix}\0{suffix}");
            Ok(())
        }
    }
}

fn devices() -> Result<()> {
    for d in list_inputs()? {
        let mark = if d.is_default { "*" } else { " " };
        println!("{mark} {}  ({})", d.name, d.id);
    }
    eprintln!("· * = system default. Pick one with: muskflow listen --mic \"<part of name>\"");
    Ok(())
}

fn threads(c: &Common) -> i32 {
    if c.threads > 0 {
        return c.threads;
    }
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8) as i32)
}

/// The cleanup LLM file: `--llm` if given, otherwise the tier's.
pub(crate) fn llm_path(c: &Common, tier: Tier) -> PathBuf {
    match &c.llm {
        Some(file) => c.models.join(file),
        None => tier.llm_path(&c.models),
    }
}

/// The ASR model directory: `--asr` if given, otherwise the tier's.
pub(crate) fn asr_dir(c: &Common) -> PathBuf {
    match &c.asr {
        Some(dir) => c.models.join(dir),
        None => c.tier.asr_path(&c.models),
    }
}

/// `--hotwords` plus the `--dictionary` file.
pub(crate) fn dictionary(c: &Common) -> Result<Dictionary> {
    let mut terms = c.hotwords.clone();
    if let Some(path) = &c.dictionary {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read dictionary {}", path.display()))?;
        terms.extend(Dictionary::parse(&text).terms().map(String::from));
    }
    Ok(Dictionary::new(terms))
}

/// Terms the speech model itself is biased towards; only with `--hotword-boost`.
fn asr_hotwords(c: &Common) -> Result<Vec<String>> {
    Ok(if c.hotword_boost > 0.0 {
        dictionary(c)?.terms().map(String::from).collect()
    } else {
        Vec::new()
    })
}

pub(crate) fn load_cleaner(c: &Common, path: &Path) -> Result<Cleaner> {
    let t = Instant::now();
    let cfg = LlmConfig {
        gpu_layers: c.gpu_layers,
        n_threads: threads(c),
        ..Default::default()
    };
    let llm = Llm::load(path, cfg)?;
    eprintln!(
        "· loaded {} in {}{}",
        llm.name(),
        ms(t.elapsed()),
        if llm.speculative() {
            " (prompt-lookup decoding)"
        } else {
            ""
        }
    );
    let cleaner = Cleaner::new(llm).with_dictionary(dictionary(c)?);
    cleaner.clean("um warm up the the model")?; // first call pays for Metal shader compilation
    Ok(cleaner)
}

fn clean(c: &Common, text: Option<&str>, style: mf_cleanup::Style) -> Result<()> {
    let text = match text {
        Some(t) => t.to_string(),
        None => std::io::read_to_string(std::io::stdin())?,
    };
    let cleaner = load_cleaner(c, &llm_path(c, c.tier))?;
    let r = cleaner.clean_as(&text, style)?;
    println!("{}", r.text);
    let mut info = format!("· {:?} in {}", r.path, ms(r.time));
    if let Some(g) = &r.generation {
        info += &format!(
            " · prompt {} tok ({} new) in {} · {} tok in {} ({} passes)",
            g.prompt_tokens,
            g.prefilled_tokens,
            ms(g.prefill_time),
            g.generated_tokens,
            ms(g.decode_time),
            g.decode_passes
        );
    }
    if let Some(v) = &r.verdict
        && !v.ok()
    {
        info += &format!(
            " · guard: {} {:?} · llm said: {}",
            v.reason.unwrap_or(""),
            v.added,
            r.llm_text.as_deref().unwrap_or("")
        );
    }
    eprintln!("{info}");
    Ok(())
}

fn list_models(models: &Path) -> Result<()> {
    for tier in Tier::ALL {
        let mark = if tier.is_installed(models) {
            "✓ installed"
        } else {
            "· not installed"
        };
        println!(
            "{:<9} {:<14} ~{:>4} MB download   {}",
            tier.name(),
            mark,
            tier.download_mb(),
            tier.asr_label()
        );
    }
    eprintln!("· install with: muskflow models install <lite|standard|max|all>");
    Ok(())
}

pub(crate) fn load_engine(c: &Common, dir: &Path) -> Result<Engine> {
    if !dir.is_dir() {
        anyhow::bail!(
            "model {} isn't installed. Run: muskflow models install {}",
            dir.display(),
            c.tier
        );
    }
    let t = Instant::now();
    let committer = CommitterConfig {
        step_secs: c.step,
        context_secs: c.context,
        ..Default::default()
    };
    let engine = Engine::load(
        dir,
        threads(c),
        &c.provider,
        asr_hotwords(c)?,
        c.hotword_boost,
    )?
    .with_committer(committer)
    .with_vad(&c.models.join("silero_vad.onnx"))?;
    engine.warm_up()?;
    eprintln!("· loaded {} in {}", engine.name(), ms(t.elapsed()));
    Ok(engine)
}

pub(crate) fn load_vad(c: &Common) -> Result<SpeechTrimmer> {
    SpeechTrimmer::load(&c.models.join("silero_vad.onnx"), VadConfig::default())
}

fn transcribe(c: &Common, wavs: &[PathBuf], no_vad: bool) -> Result<()> {
    let engine = load_engine(c, &asr_dir(c))?;
    let asr = engine
        .transcriber()
        .context("native streaming models can't decode a file in one pass; use `stream`")?;
    let vad = if no_vad { None } else { Some(load_vad(c)?) };
    let hotwords = asr_hotwords(c)?;
    let dict = dictionary(c)?;
    let batch = wavs.len() > 1;
    for wav in wavs {
        let pcm = load_wav_16k(wav)?;
        let t = Instant::now();
        let speech = match &vad {
            Some(v) => v.trim(&pcm),
            None => &pcm[..],
        };
        let vad_time = t.elapsed();
        if speech.is_empty() {
            if batch {
                println!("{}\t", wav.display());
            } else {
                eprintln!("· no speech detected");
            }
            continue;
        }
        if batch {
            let hyp = asr.transcribe(speech, &hotwords)?;
            println!("{}\t{}", wav.display(), dict.apply(&hyp.text()));
            continue;
        }

        // One warm-up decode so the timing reflects steady state, not first-call allocation.
        asr.transcribe(&speech[..speech.len().min(16_000)], &hotwords)?;
        let t = Instant::now();
        let hyp = asr.transcribe(speech, &hotwords)?;
        let dt = t.elapsed();

        println!("{}", dict.apply(&hyp.text()));
        eprintln!(
            "· audio {:.2}s → speech {:.2}s · vad {} · decode {} · {:.0}× real time",
            samples_to_secs(pcm.len()),
            samples_to_secs(speech.len()),
            ms(vad_time),
            ms(dt),
            samples_to_secs(speech.len()) / dt.as_secs_f32()
        );
    }
    Ok(())
}

fn stream(c: &Common, wav: &Path, fast: bool) -> Result<()> {
    let engine = load_engine(c, &asr_dir(c))?;
    let pcm = load_wav_16k(wav)?;
    let r = replay(&engine, &pcm, fast, true)?;
    println!("\n\n{}", dictionary(c)?.apply(&r.text));
    eprintln!(
        "· audio {:.2}s · {} decodes ({} total, {:.0}% of audio time) · tail {:.2}s in {} · key-up → text {}",
        samples_to_secs(pcm.len()),
        r.stats.decodes,
        ms(r.stats.decode_time),
        100.0 * r.cpu_share(),
        r.stats.tail_secs,
        ms(r.stats.tail_time),
        ms(r.latency)
    );
    Ok(())
}

pub(crate) struct Replay {
    pub text: String,
    pub stats: SessionStats,
    /// Key-up → final text.
    pub latency: Duration,
    pub audio_secs: f32,
}

impl Replay {
    /// Decode time as a share of audio time: roughly the CPU the ASR keeps busy while you talk.
    pub fn cpu_share(&self) -> f32 {
        self.stats.decode_time.as_secs_f32() / self.audio_secs.max(1e-3)
    }
}

/// Feeds a recording through a session in 100 ms chunks, as the mic would, then "releases the key".
pub(crate) fn replay(engine: &Engine, pcm: &[f32], fast: bool, print: bool) -> Result<Replay> {
    let mut session = engine.session()?;
    const CHUNK: usize = 1_600; // 100 ms, roughly what the mic pump delivers
    let start = Instant::now();
    for (i, chunk) in pcm.chunks(CHUNK).enumerate() {
        if !fast {
            // Hold to the real-time schedule; if a decode overran, we're simply behind (as live would be).
            let due = Duration::from_secs_f32((i * CHUNK) as f32 / 16_000.0);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let words = session.push(chunk)?;
        if print {
            print_committed(&words);
        }
    }
    // "Key up": everything after this is user-visible latency.
    let key_up = Instant::now();
    let (hyp, stats) = session.finish()?;
    Ok(Replay {
        text: hyp.text(),
        stats,
        latency: key_up.elapsed(),
        audio_secs: samples_to_secs(pcm.len()),
    })
}

/// One dictation from the mic. Decodes while audio arrives, until `wait` returns: `true`
/// to finish and keep the transcript, `false` to discard it. With `live`, words are
/// printed to stderr as they become final.
pub(crate) fn record(
    engine: &Engine,
    device: Option<&str>,
    live: bool,
    wait: impl FnOnce(&MicCapture) -> Result<bool>,
) -> Result<Option<Recording>> {
    let (tx, rx) = crossbeam_channel::unbounded::<Vec<f32>>();
    let mic = MicCapture::start(device, tx).context("could not open the microphone")?;
    let stopping = AtomicBool::new(false);
    std::thread::scope(|s| -> Result<_> {
        let worker = s.spawn(|| -> Result<_> {
            let mut session = engine.session()?;
            let mut all = Vec::new();
            // The channel closes once the mic is stopped and the pump has flushed.
            // Coalesce everything queued into one push, so a slow decode is followed by one
            // catch-up decode instead of a backlog of them.
            let mut batch = Vec::new();
            for chunk in rx.iter() {
                batch.extend_from_slice(&chunk);
                batch.extend(rx.try_iter().flatten());
                all.extend_from_slice(&batch);
                if stopping.load(Ordering::Acquire) {
                    // Key is up: the final decode covers this, so don't spend a decode on it now.
                    session.append(&batch);
                } else {
                    let words = session.push(&batch)?;
                    if live {
                        print_committed(&words);
                    }
                }
                batch.clear();
            }
            let key_up = Instant::now();
            let (hyp, stats) = session.finish()?;
            Ok((hyp, stats, key_up.elapsed(), all))
        });

        let keep = wait(&mic);
        let stopped = Instant::now();
        stopping.store(true, Ordering::Release);
        let peak = mic.peak();
        mic.stop();
        let (hyp, stats, finish, audio) = worker.join().expect("ASR worker panicked")?;
        Ok(keep?.then_some(Recording {
            hyp,
            stats,
            finish,
            audio,
            peak,
            stopped,
        }))
    })
}

pub(crate) struct Recording {
    pub hyp: Hypothesis,
    pub stats: SessionStats,
    /// Time from key-up to the final transcript.
    pub finish: Duration,
    pub audio: Vec<f32>,
    pub peak: f32,
    /// When the user stopped speaking (key-up).
    pub stopped: Instant,
}

impl Recording {
    /// Why there's nothing to insert, if anything: a silent mic or no speech.
    pub fn problem(&self, vad: &SpeechTrimmer) -> Option<String> {
        let peak_db = 20.0 * self.peak.max(1e-9).log10();
        if self.peak < 1e-3 {
            return Some(format!(
                "the mic sent silence (peak {peak_db:.0} dBFS). Check it isn't muted, or pick another with --mic (see `muskflow devices`).\n  Bluetooth headsets often deliver nothing until they switch to headset mode."
            ));
        }
        if vad.speech_bounds(&self.audio).is_none() {
            return Some(format!("no speech detected (mic peak {peak_db:.0} dBFS)"));
        }
        None
    }
}

fn listen(c: &Common, device: Option<&str>, save: Option<&Path>) -> Result<()> {
    let engine = load_engine(c, &asr_dir(c))?;
    let vad = load_vad(c)?;
    let r = record(&engine, device, true, |mic| {
        eprintln!(
            "· mic \"{}\" ({}) @ {} Hz × {} ch — speak, then press Enter",
            mic.info.name, mic.info.device, mic.info.sample_rate, mic.info.channels
        );
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        Ok(true)
    })?
    .expect("listen always keeps the recording");
    if let Some(path) = save {
        save_wav_16k(path, &r.audio)?;
        eprintln!("· saved recording to {}", path.display());
    }
    if let Some(problem) = r.problem(&vad) {
        eprintln!("· {problem}");
        return Ok(());
    }
    println!("\n{}", dictionary(c)?.apply(&r.hyp.text()));
    eprintln!(
        "· spoke {:.2}s · {} decodes · tail {:.2}s in {} · Enter → text {}",
        samples_to_secs(r.audio.len()),
        r.stats.decodes,
        r.stats.tail_secs,
        ms(r.finish),
        ms(r.stopped.elapsed())
    );
    Ok(())
}

fn print_committed(words: &[Word]) {
    if words.is_empty() {
        return;
    }
    let mut out = std::io::stderr().lock();
    for w in words {
        let _ = write!(out, "{} ", w.text);
    }
    let _ = out.flush();
}

pub(crate) fn ms(d: Duration) -> String {
    format!("{:.0}ms", d.as_secs_f64() * 1000.0)
}
