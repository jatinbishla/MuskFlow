//! `muskflow app` (and MuskFlow.app): hold Fn anywhere, speak, let go, and the
//! cleaned text is typed at your cursor.
//!
//! A menu-bar icon shows the state, a pill near the bottom of the screen shows
//! "Listening" while Fn is held, and a sound marks start and finish. Models load
//! once and stay warm; the microphone is open only while Fn is held. The user's
//! dictionary lives in a plain text file and is picked up as soon as it's saved.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result};
use mf_audio::samples_to_secs;
use mf_cleanup::{Cleaner, Style};
use mf_context::Focus;
use mf_hotkey::{Event, Mode};
use mf_models::Tier;
use mf_shell::{State, Ui};

use crate::history::{Entry, History};
use crate::settings::Settings;
use crate::{
    Common, asr_dir, dictionary, llm_path, load_cleaner, load_engine, load_vad, ms, record,
};

/// Shorter presses are accidental taps, not dictation.
const MIN_SPEECH_SECS: f32 = 0.3;

const DICTIONARY_TEMPLATE: &str = "\
# MuskFlow dictionary: one name or term per line, spelled the way you want it typed.
# MuskFlow fixes words that sound like these but come out spelled wrong
# (\"Subflow\" -> SubFlo, \"firman\" -> foreman). Saved changes apply to the next dictation.
#
# Examples:
# SubFlo
# Jatin Bishla
# lien waivers
";

/// Whether this binary is running as MuskFlow.app (double-clicked, no arguments).
pub fn in_app_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

/// A double-clicked app has no terminal: send stderr to `~/Library/Logs/MuskFlow.log`.
pub fn log_to_file() {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let path = home.join("Library/Logs/MuskFlow.log");
    #[cfg(target_os = "macos")]
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::os::fd::AsRawFd;
        // SAFETY: dup2 on two valid descriptors; the file stays open for the process lifetime.
        unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
        std::mem::forget(file);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// `~/Library/Application Support/MuskFlow/dictionary.txt`.
fn default_dictionary() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/MuskFlow/dictionary.txt")
}

pub fn run(c: &Common, device: Option<&str>) -> Result<()> {
    let mut c = c.clone();
    let settings = Settings::load();
    // A tier picked in the window wins over the one the app was built with, if installed.
    if let Some(tier) = settings.tier().filter(|t| t.is_installed(&c.models)) {
        c.tier = tier;
    }
    let dict_path = c.dictionary.clone().unwrap_or_else(default_dictionary);
    if !dict_path.exists() {
        if let Some(dir) = dict_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&dict_path, DICTIONARY_TEMPLATE)
            .with_context(|| format!("could not create {}", dict_path.display()))?;
    }
    c.dictionary = Some(dict_path.clone());
    let device = device.map(String::from);
    let hub = crate::hub::Hub {
        history: History::default_path(),
        dictionary: dict_path.clone(),
        tier: c.tier,
        models: c.models.clone(),
        settings: settings.clone(),
    };
    let config = mf_shell::Config {
        dictionary: dict_path,
        tier: c.tier.to_string(),
        html: include_str!("ui/index.html"),
        backend: hub.into_backend(),
        // Double-clicked: show the window, like any app. From a terminal: stay in the menu bar.
        open_window: in_app_bundle(),
    };
    mf_shell::run(config, move |ui| {
        if let Err(e) = dictate_loop(&c, device.as_deref(), &settings, &ui) {
            eprintln!("Error: {e:#}");
            ui.set(State::Error(format!("{e:#}")));
        }
    })
}

/// The style for this dictation: the user's fixed choice, or the one the focused app calls
/// for. Lite's 0.8B model can't rewrite safely yet, so it keeps rewriting styles to Clean.
fn pick_style(c: &Common, settings: &Settings, focus: Option<&Focus>) -> Style {
    let style = settings
        .style()
        .unwrap_or_else(|| focus.map(Focus::style).unwrap_or_default());
    if c.tier == Tier::Lite && style.rewrites() {
        Style::Clean
    } else {
        style
    }
}

fn dictate_loop(c: &Common, device: Option<&str>, settings: &Settings, ui: &Ui) -> Result<()> {
    // Load first, so dictation works the moment setup is finished.
    let engine = load_engine(c, &asr_dir(c))?;
    let vad = load_vad(c)?;
    let dict_path = c
        .dictionary
        .clone()
        .expect("run() sets the dictionary path");
    let mut dict = dictionary(c)?;
    let mut dict_stamp = modified(&dict_path);
    let mut cleaner: Option<Cleaner> = if llm_path(c, c.tier).is_file() {
        Some(load_cleaner(c, &llm_path(c, c.tier))?)
    } else {
        eprintln!(
            "· no cleanup model for the {} tier; using rules only (muskflow models install {})",
            c.tier, c.tier
        );
        None
    };
    let events = start_hotkey(ui)?;
    eprintln!(
        "· ready ({} tier). Hold Fn and speak; let go to type it. Fn + Control: command mode. Dictionary: {}",
        c.tier,
        dict_path.display()
    );
    let history = History::new(History::default_path());
    ui.set(State::Ready);

    loop {
        let Event::Down(mode) = events.recv()? else {
            continue;
        };
        // Where the text will go, read now, while that app is still in front.
        let focus = mf_context::focused();
        let style = pick_style(c, settings, focus.as_ref());
        ui.set(State::Listening {
            command: mode == Mode::Command,
            style: style.label().into(),
        });
        eprint!(
            "● listening{} ",
            if mode == Mode::Command {
                " (command)"
            } else {
                ""
            }
        );
        let mut final_mode = mode;
        let rec = record(&engine, device, false, |_| {
            // Wait for Fn to come up, or for another key to cancel.
            loop {
                match events.recv()? {
                    Event::Up(m) => {
                        final_mode = m;
                        return Ok(true);
                    }
                    Event::Cancel => return Ok(false),
                    Event::Down(_) => {}
                }
            }
        });
        let rec = match rec {
            Ok(Some(r)) => r,
            Ok(None) => {
                eprintln!("→ cancelled");
                ui.set(State::Nothing("Cancelled".into()));
                continue;
            }
            Err(e) => {
                eprintln!("→ {e:#}");
                ui.set(State::Error(format!("{e:#}")));
                continue;
            }
        };
        ui.set(State::Working);
        let spoke = samples_to_secs(rec.audio.len());
        if spoke < MIN_SPEECH_SECS {
            eprintln!("→ too short");
            ui.set(State::Nothing("Hold Fn while you speak".into()));
            continue;
        }
        if let Some(problem) = rec.problem(&vad) {
            eprintln!("→ {problem}");
            ui.set(State::Nothing("No speech heard".into()));
            continue;
        }
        if final_mode == Mode::Command {
            eprint!("(command mode isn't built yet; typing it as dictation) ");
        }

        // Pick up dictionary edits without a restart.
        let stamp = modified(&dict_path);
        if stamp != dict_stamp {
            dict_stamp = stamp;
            match dictionary(c) {
                Ok(d) => {
                    dict = d;
                    if let Some(cl) = cleaner.as_mut() {
                        cl.set_dictionary(dict.clone());
                    }
                    eprint!("(dictionary reloaded) ");
                }
                Err(e) => eprint!("(dictionary not reloaded: {e:#}) "),
            }
        }

        let raw = rec.hyp.text();
        let t = Instant::now();
        let text = match &cleaner {
            Some(cl) => match cl.clean_as(&raw, style) {
                Ok(cd) => cd.text,
                Err(e) => {
                    log::warn!("cleanup failed: {e:#}");
                    mf_cleanup::rules::clean(&dict.apply(&raw))
                }
            },
            None => mf_cleanup::rules::clean(&dict.apply(&raw)),
        };
        let clean_time = t.elapsed();
        if text.is_empty() {
            eprintln!("→ nothing to type");
            ui.set(State::Nothing("Nothing to type".into()));
            continue;
        }
        // A trailing space so the next dictation doesn't run into this one.
        match mf_insert::insert(&format!("{text} ")) {
            Ok(()) => {
                let latency = rec.stopped.elapsed();
                let entry = Entry {
                    at: History::now_ms(),
                    text: text.clone(),
                    raw: raw.clone(),
                    app: focus.as_ref().map(Focus::label),
                    style: Some(style.name().into()),
                    mode: if final_mode == Mode::Command {
                        "command"
                    } else {
                        "dictate"
                    }
                    .into(),
                    spoke_secs: spoke,
                    latency_ms: latency.as_millis() as u32,
                };
                if let Err(e) = history.append(&entry) {
                    log::warn!("could not save history: {e:#}");
                }
                ui.set(State::Typed);
                eprintln!(
                    "→ typed in {} (speech {} + cleanup {}) · spoke {spoke:.1}s\n  {text}",
                    ms(rec.stopped.elapsed()),
                    ms(rec.finish),
                    ms(clean_time)
                );
            }
            Err(e) => {
                eprintln!("→ {e:#}\n  {text}");
                ui.set(State::Error(format!("{e:#}")));
            }
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Starts the Fn watcher once Input Monitoring is allowed. Inside MuskFlow.app the setup
/// screen walks the user through it, so this waits instead of failing; from a terminal it
/// asks for the permissions and explains where they go.
fn start_hotkey(ui: &Ui) -> Result<crossbeam_channel::Receiver<Event>> {
    if !in_app_bundle() {
        if !mf_insert::has_permission() {
            mf_insert::request_permission();
            eprintln!(
                "· MuskFlow needs Accessibility to type into other apps: System Settings → Privacy & Security → Accessibility."
            );
        }
        if !mf_hotkey::has_permission() {
            mf_hotkey::request_permission();
            anyhow::bail!(
                "Allow Input Monitoring for the app you run MuskFlow in (System Settings → Privacy & Security → Input Monitoring), then restart that app. Use macOS Terminal: apps that host a terminal, like Claude, need a full restart."
            );
        }
    }
    if !mf_hotkey::has_permission() {
        ui.set(State::Setup);
        eprintln!("· waiting for Input Monitoring (finish setup in the MuskFlow window)");
        while !mf_hotkey::has_permission() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    mf_hotkey::start()
}
