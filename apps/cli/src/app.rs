//! `umless app` (and Umless.app): hold Fn anywhere, speak, let go, and the
//! cleaned text is typed at your cursor.
//!
//! A menu-bar icon shows the state, a pill near the bottom of the screen shows
//! "Listening" while Fn is held, and a sound marks start and finish. Models load
//! once and stay warm; the microphone is open only while Fn is held. The user's
//! dictionary lives in a plain text file and is picked up as soon as it's saved.
//!
//! Fn + Control is command mode: say what to write ("reply saying I'll send it
//! Friday", or "make this shorter" with text selected). While the user speaks, the
//! text on screen is read in the background (umless-context): it gives command mode its
//! context, spells names the way the screen does, and sets the tone. After typing,
//! the field is watched for a minute and a half so fixed names can be learned.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use anyhow::{Context, Result};
use umless_audio::samples_to_secs;
use umless_cleanup::{Cleaner, Style};
use umless_context::{Focus, Screen};
use umless_hotkey::{Event, Mode};
use umless_models::Tier;
use umless_shell::{State, Ui};

use crate::history::{Entry, History};
use crate::learn::{self, Pending};
use crate::settings::Settings;
use crate::{
    Common, asr_dir, dictionary, llm_path, load_cleaner, load_engine, load_vad, ms, record,
};

/// Shorter presses are accidental taps, not dictation.
const MIN_SPEECH_SECS: f32 = 0.3;

const DICTIONARY_TEMPLATE: &str = "\
# Umless dictionary: one name or term per line, spelled the way you want it typed.
# Umless fixes words that sound like these but come out spelled wrong
# (\"Subflow\" -> SubFlo, \"firman\" -> foreman). Saved changes apply to the next dictation.
#
# Examples:
# SubFlo
# Jatin Bishla
# lien waivers
#
# If a word keeps coming out as something else, add what you hear after \"=\":
# Gaurav = Gorov, Barraf
";

/// Whether this binary is running as Umless.app (double-clicked, no arguments).
pub fn in_app_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

/// A double-clicked app has no terminal: send stderr to the log file (`paths::log_path`).
pub fn log_to_file() {
    let path = crate::paths::log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    #[cfg(unix)]
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
    #[cfg(not(unix))]
    let _ = path;
}

/// `dictionary.txt` in the data folder (`paths::data_dir`).
fn default_dictionary() -> PathBuf {
    crate::paths::data_dir().join("dictionary.txt")
}

/// Umless was called MuskFlow: carry the user's history, dictionary and settings over.
fn move_old_data() {
    let new = crate::paths::data_dir();
    let Some(old) = new.parent().map(|p| p.join("MuskFlow")) else {
        return;
    };
    if old.is_dir() && !new.exists() {
        match std::fs::rename(&old, &new) {
            Ok(()) => eprintln!(
                "· moved your data from {} to {}",
                old.display(),
                new.display()
            ),
            Err(e) => eprintln!("· could not move {}: {e}", old.display()),
        }
    }
}

pub fn run(c: &Common, device: Option<&str>) -> Result<()> {
    move_old_data();
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
    let config = umless_shell::Config {
        dictionary: dict_path,
        tier: c.tier.to_string(),
        html: include_str!("ui/index.html"),
        backend: hub.into_backend(),
        // Double-clicked: show the window, like any app. From a terminal: stay in the menu bar.
        open_window: in_app_bundle(),
    };
    umless_shell::run(config, move |ui| {
        if let Err(e) = dictate_loop(&c, device.as_deref(), &settings, &ui) {
            eprintln!("Error: {e:#}");
            ui.set(State::Error(format!("{e:#}")));
        }
    })
}

/// The style for this dictation: the user's fixed choice, or the one the focused app calls
/// for. Lite's 0.8B model can't rewrite safely yet, so it keeps rewriting styles to Clean.
fn pick_style(
    c: &Common,
    settings: &Settings,
    focus: Option<&Focus>,
    screen: Option<&Screen>,
) -> Style {
    let style = settings.style().unwrap_or_else(|| match (focus, screen) {
        (Some(f), Some(sc)) => f.style_for(sc),
        (Some(f), None) => f.style(),
        _ => Style::default(),
    });
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
            "· no cleanup model for the {} tier; using rules only (umless models install {})",
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

    let mut pending: Option<Pending> = None;

    loop {
        // Between dictations, look back at the last one for corrections when a check is due.
        let event = match pending.as_ref().and_then(Pending::next_check) {
            Some(due) => match events.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(ev) => Some(ev),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(e) => return Err(e.into()),
            },
            None => Some(events.recv()?),
        };
        let Some(Event::Down(mode)) = event else {
            if event.is_none() {
                check_corrections(&mut pending, &mut dict, &dict_path, ui);
            }
            continue;
        };
        // Last chance to see the user's fixes before they move on.
        if pending.is_some() {
            check_corrections(&mut pending, &mut dict, &dict_path, ui);
            pending = None;
        }
        // Where the text will go, read now, while that app is still in front.
        let focus = umless_context::focused();
        // The text on screen, read while the user speaks.
        let screen_reader = focus
            .as_ref()
            .filter(|_| settings.screen_context() || mode == Mode::Command)
            .map(|f| {
                let pid = f.pid;
                std::thread::spawn(move || umless_context::screen(pid))
            });
        let style = pick_style(c, settings, focus.as_ref(), None);
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
        // Only the latest take, in the temp folder, so a bad transcript can be replayed with `umless stream`.
        if let Err(e) = umless_audio::save_wav_16k(&last_take(), &rec.audio) {
            log::warn!("could not keep the recording: {e:#}");
        }
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
        let screen = screen_reader.and_then(|r| r.join().ok());

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
        if final_mode == Mode::Command {
            let Some(cl) = cleaner.as_ref() else {
                ui.set(State::Error(format!(
                    "Command mode needs the {} tier's language model: umless models install {}",
                    c.tier, c.tier
                )));
                continue;
            };
            let screen = screen.unwrap_or_default();
            run_command(
                cl,
                &raw,
                &screen,
                focus.as_ref(),
                settings,
                &history,
                spoke,
                &rec.stopped,
                ui,
            );
            continue;
        }
        let screen = screen.filter(|_| settings.screen_context());
        let style = pick_style(c, settings, focus.as_ref(), screen.as_ref());
        // Names on screen ("Priya" in the email thread) spell the words that sound like them.
        if let Some(cl) = cleaner.as_mut() {
            match &screen {
                Some(sc) => cl.set_dictionary(
                    dict.with_screen_terms(umless_context::text::names(&sc.all_text())),
                ),
                None => cl.set_dictionary(dict.clone()),
            }
        }
        let t = Instant::now();
        let text = match &cleaner {
            Some(cl) => match cl.clean_as(&raw, style) {
                Ok(cd) => cd.text,
                Err(e) => {
                    log::warn!("cleanup failed: {e:#}");
                    umless_cleanup::without_llm(&raw, &dict)
                }
            },
            None => umless_cleanup::without_llm(&raw, &dict),
        };
        let clean_time = t.elapsed();
        if text.is_empty() {
            eprintln!("→ nothing to type");
            ui.set(State::Nothing("Nothing to type".into()));
            continue;
        }
        // A trailing space so the next dictation doesn't run into this one.
        match umless_insert::insert(&format!("{text} ")) {
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
                if settings.learning()
                    && let Some(f) = &focus
                {
                    pending = Some(Pending {
                        pid: f.pid,
                        typed: text.clone(),
                        at: Instant::now(),
                        checks: 0,
                    });
                }
                eprintln!(
                    "→ typed in {} (speech {} + cleanup {}) · spoke {spoke:.1}s{}\n  {text}",
                    ms(rec.stopped.elapsed()),
                    ms(rec.finish),
                    ms(clean_time),
                    match rec.stats.rescued {
                        0 => String::new(),
                        n => format!(" · {n} missed stretch(es) decoded again"),
                    } + &match rec.dead_start() {
                        // Bluetooth mics can take a moment to wake; anything said before that is gone.
                        s if s >= 0.5 => format!(" · mic sent nothing for the first {s:.1}s"),
                        _ => String::new(),
                    }
                );
            }
            Err(e) => {
                eprintln!("→ {e:#}\n  {text}");
                ui.set(State::Error(format!("{e:#}")));
            }
        }
    }
}

/// Fn + Control: carries out the spoken instruction and types (or replaces the selection
/// with) the result.
#[allow(clippy::too_many_arguments)]
fn run_command(
    cleaner: &Cleaner,
    raw: &str,
    screen: &Screen,
    focus: Option<&Focus>,
    settings: &Settings,
    history: &History,
    spoke: f32,
    stopped: &Instant,
    ui: &Ui,
) {
    // Apps that don't expose their text field: ask them for the selection with ⌘C.
    let copied = if screen.selection.is_none() && !screen.readable {
        umless_insert::copy_selection()
    } else {
        None
    };
    let selection = screen.selection.as_deref().or(copied.as_deref());
    let app = focus.map(Focus::label);
    // Only the tone matters here, so Lite keeps the destination's real style.
    let style = settings
        .style()
        .unwrap_or_else(|| focus.map(|f| f.style_for(screen)).unwrap_or_default());
    let cx = umless_cleanup::command::Context {
        app: app.as_deref(),
        style,
        selection,
        before: &screen.before,
        window: &screen.window,
    };
    let out = match cleaner.command(raw, &cx) {
        Ok(out) if !out.text.is_empty() => out,
        Ok(_) => {
            eprintln!("→ command gave nothing to type");
            ui.set(State::Nothing("Nothing to write".into()));
            return;
        }
        Err(e) => {
            eprintln!("→ command failed: {e:#}");
            ui.set(State::Error(format!("{e:#}")));
            return;
        }
    };
    // Replacing a selection: paste over it. New text: paste at the cursor.
    if let Err(e) = umless_insert::insert(&out.text) {
        eprintln!("→ {e:#}\n  {}", out.text);
        ui.set(State::Error(format!("{e:#}")));
        return;
    }
    let entry = Entry {
        at: History::now_ms(),
        text: out.text.clone(),
        raw: raw.to_string(),
        app,
        style: Some(style.name().into()),
        mode: "command".into(),
        spoke_secs: spoke,
        latency_ms: stopped.elapsed().as_millis() as u32,
    };
    if let Err(e) = history.append(&entry) {
        log::warn!("could not save history: {e:#}");
    }
    ui.set(State::Typed);
    eprintln!(
        "→ command ({}) done in {} · {} chars of screen context\n  {}",
        if out.replaces_selection {
            "replaced selection"
        } else {
            "inserted"
        },
        ms(stopped.elapsed()),
        screen.window.len() + screen.before.len(),
        out.text
    );
}

/// Looks at the field Umless last typed into; words the user fixed go into the dictionary.
fn check_corrections(
    pending: &mut Option<Pending>,
    dict: &mut umless_cleanup::Dictionary,
    path: &Path,
    ui: &Ui,
) {
    let Some(p) = pending.as_mut() else {
        return;
    };
    p.checks += 1;
    let Some(field) = umless_context::field_text(p.pid) else {
        return;
    };
    let words = learn::corrections(&p.typed, &field, dict);
    if words.is_empty() {
        if p.next_check().is_none() {
            *pending = None;
        }
        return;
    }
    match learn::save(path, &words) {
        Ok(()) => {
            // The dictionary file's change is picked up before the next dictation.
            *dict = dict.with_terms(&words);
            eprintln!("· learned from your correction: {}", words.join(", "));
            ui.set(State::Nothing(format!(
                "Added {} to your dictionary",
                quote_list(&words)
            )));
        }
        Err(e) => log::warn!("could not save learned words: {e:#}"),
    }
    *pending = None;
}

fn quote_list(words: &[String]) -> String {
    words
        .iter()
        .map(|w| format!("\u{201c}{w}\u{201d}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where the latest dictation's audio is kept (overwritten each time).
pub fn last_take() -> PathBuf {
    std::env::temp_dir().join("umless-last-take.wav")
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Starts the Fn watcher once Input Monitoring is allowed. Inside Umless.app the setup
/// screen walks the user through it, so this waits instead of failing; from a terminal it
/// asks for the permissions and explains where they go.
fn start_hotkey(ui: &Ui) -> Result<crossbeam_channel::Receiver<Event>> {
    if !in_app_bundle() {
        if !umless_insert::has_permission() {
            umless_insert::request_permission();
            eprintln!(
                "· Umless needs Accessibility to type into other apps: System Settings → Privacy & Security → Accessibility."
            );
        }
        if !umless_hotkey::has_permission() {
            umless_hotkey::request_permission();
            anyhow::bail!(
                "Allow Input Monitoring for the app you run Umless in (System Settings → Privacy & Security → Input Monitoring), then restart that app. Use macOS Terminal: apps that host a terminal, like Claude, need a full restart."
            );
        }
    }
    if !umless_hotkey::has_permission() {
        ui.set(State::Setup);
        eprintln!("· waiting for Input Monitoring (finish setup in the Umless window)");
        while !umless_hotkey::has_permission() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    umless_hotkey::start()
}
