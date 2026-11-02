//! `freeflow` — headless FreeFlow for development, benchmarking and scripting.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ff_asr::{CommitterConfig, Engine, ParakeetConfig, ParakeetTranscriber, Transcriber, Word};
use ff_audio::{MicCapture, SpeechTrimmer, VadConfig, load_wav_16k, samples_to_secs};

#[derive(Parser)]
#[command(
    name = "freeflow",
    version,
    about = "Local voice dictation that writes what you meant"
)]
struct Cli {
    #[command(flatten)]
    common: Common,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args)]
struct Common {
    /// Directory with downloaded models (see scripts/fetch-models.sh).
    #[arg(long, env = "FREEFLOW_MODELS", default_value = "models", global = true)]
    models: PathBuf,
    /// ASR model directory name inside --models.
    #[arg(
        long,
        default_value = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        global = true
    )]
    asr: String,
    /// Inference threads; 0 = all cores, capped at 8.
    #[arg(long, default_value_t = 0, global = true)]
    threads: i32,
    /// Inference provider: cpu or coreml.
    #[arg(long, default_value = "cpu", global = true)]
    provider: String,
    /// Comma-separated words to bias recognition towards (switches to beam search).
    #[arg(long, value_delimiter = ',', global = true)]
    hotwords: Vec<String>,
    /// Seconds of new audio between re-decodes while speaking (offline models).
    #[arg(long, default_value_t = 1.0, global = true)]
    step: f32,
    /// Seconds of already-committed audio re-fed as left context (offline models).
    #[arg(long, default_value_t = 1.0, global = true)]
    context: f32,
}

#[derive(Subcommand)]
enum Cmd {
    /// Transcribe a WAV file in one pass.
    Transcribe {
        wav: PathBuf,
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
    Listen,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Transcribe { wav, no_vad } => transcribe(&cli.common, wav, *no_vad),
        Cmd::Stream { wav, fast } => stream(&cli.common, wav, *fast),
        Cmd::Listen => listen(&cli.common),
    }
}

fn threads(c: &Common) -> i32 {
    if c.threads > 0 {
        return c.threads;
    }
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8) as i32)
}

fn load_asr(c: &Common) -> Result<ParakeetTranscriber> {
    let t = Instant::now();
    let mut cfg = ParakeetConfig::new(c.models.join(&c.asr));
    cfg.num_threads = threads(c);
    cfg.provider = c.provider.clone();
    cfg.hotwords = !c.hotwords.is_empty();
    let asr = ParakeetTranscriber::load(&cfg)?;
    eprintln!("· loaded {} in {}", asr.name(), ms(t.elapsed()));
    Ok(asr)
}

fn load_engine(c: &Common) -> Result<Engine> {
    let t = Instant::now();
    let committer = CommitterConfig {
        step_secs: c.step,
        context_secs: c.context,
        ..Default::default()
    };
    let engine = Engine::load(
        &c.models.join(&c.asr),
        threads(c),
        &c.provider,
        c.hotwords.clone(),
    )?
    .with_committer(committer);
    engine.warm_up()?;
    eprintln!("· loaded {} in {}", engine.name(), ms(t.elapsed()));
    Ok(engine)
}

fn load_vad(c: &Common) -> Result<SpeechTrimmer> {
    SpeechTrimmer::load(&c.models.join("silero_vad.onnx"), VadConfig::default())
}

fn transcribe(c: &Common, wav: &Path, no_vad: bool) -> Result<()> {
    let asr = load_asr(c)?;
    let pcm = load_wav_16k(wav)?;
    let t = Instant::now();
    let speech = if no_vad {
        &pcm[..]
    } else {
        load_vad(c)?.trim(&pcm)
    };
    let vad_time = t.elapsed();
    if speech.is_empty() {
        eprintln!("· no speech detected");
        return Ok(());
    }

    // One warm-up decode so the timing reflects steady state, not first-call allocation.
    asr.transcribe(&speech[..speech.len().min(16_000)], &c.hotwords)?;
    let t = Instant::now();
    let hyp = asr.transcribe(speech, &c.hotwords)?;
    let dt = t.elapsed();

    println!("{}", hyp.text());
    let audio = samples_to_secs(pcm.len());
    eprintln!(
        "· audio {:.2}s → speech {:.2}s · vad {} · decode {} · {:.0}× real time",
        audio,
        samples_to_secs(speech.len()),
        ms(vad_time),
        ms(dt),
        samples_to_secs(speech.len()) / dt.as_secs_f32()
    );
    Ok(())
}

fn stream(c: &Common, wav: &Path, fast: bool) -> Result<()> {
    let engine = load_engine(c)?;
    let pcm = load_wav_16k(wav)?;
    let mut session = engine.session();

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
        print_committed(&session.push(chunk)?);
    }

    // "Key up": everything after this is user-visible latency.
    let key_up = Instant::now();
    let (hyp, stats) = session.finish()?;
    let latency = key_up.elapsed();
    println!("\n\n{}", hyp.text());
    eprintln!(
        "· audio {:.2}s · {} decodes ({} total, {:.0}% of audio time) · tail {:.2}s in {} · key-up → text {}",
        samples_to_secs(pcm.len()),
        stats.decodes,
        ms(stats.decode_time),
        100.0 * stats.decode_time.as_secs_f32() / samples_to_secs(pcm.len()),
        stats.tail_secs,
        ms(stats.tail_time),
        ms(latency)
    );
    Ok(())
}

fn listen(c: &Common) -> Result<()> {
    let engine = load_engine(c)?;
    let vad = load_vad(c)?;

    let (tx, rx) = crossbeam_channel::unbounded::<Vec<f32>>();
    let mic = MicCapture::start(tx).context("could not open the microphone")?;
    eprintln!(
        "· mic {} @ {} Hz × {} ch — speak, then press Enter",
        mic.info.device, mic.info.sample_rate, mic.info.channels
    );

    let stopping = AtomicBool::new(false);
    std::thread::scope(|s| -> Result<()> {
        let worker = s.spawn(|| -> Result<_> {
            let mut session = engine.session();
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
                    print_committed(&session.push(&batch)?);
                }
                batch.clear();
            }
            let key_up = Instant::now();
            let (hyp, stats) = session.finish()?;
            Ok((hyp, stats, key_up.elapsed(), all))
        });

        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        let stop = Instant::now();
        stopping.store(true, Ordering::Release);
        mic.stop();
        let (hyp, stats, finish, all) = worker.join().expect("ASR worker panicked")?;

        if vad.speech_bounds(&all).is_none() {
            eprintln!("· no speech detected");
            return Ok(());
        }
        println!("\n{}", hyp.text());
        eprintln!(
            "· spoke {:.2}s · {} decodes · tail {:.2}s in {} · Enter → text {}",
            samples_to_secs(all.len()),
            stats.decodes,
            stats.tail_secs,
            ms(finish),
            ms(stop.elapsed())
        );
        Ok(())
    })
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

fn ms(d: Duration) -> String {
    format!("{:.0}ms", d.as_secs_f64() * 1000.0)
}
