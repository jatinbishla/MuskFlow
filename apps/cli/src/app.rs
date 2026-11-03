//! `muskflow app`: hold Fn anywhere, speak, let go, and the cleaned text is typed
//! at your cursor.
//!
//! A terminal-only first version of the desktop app (ARCHITECTURE.md §14): no
//! menu bar or overlay yet. Models load once and stay warm; the microphone is
//! open only while Fn is held.

use std::time::Instant;

use anyhow::Result;
use mf_audio::samples_to_secs;
use mf_cleanup::Cleaner;
use mf_hotkey::{Event, Mode};

use crate::{
    Common, asr_dir, dictionary, llm_path, load_cleaner, load_engine, load_vad, ms, record,
};

/// Shorter presses are accidental taps, not dictation.
const MIN_SPEECH_SECS: f32 = 0.3;

pub fn run(c: &Common, device: Option<&str>) -> Result<()> {
    check_permissions();

    let engine = load_engine(c, &asr_dir(c))?;
    let vad = load_vad(c)?;
    let dict = dictionary(c)?;
    let cleaner: Option<Cleaner> = if llm_path(c, c.tier).is_file() {
        Some(load_cleaner(c, &llm_path(c, c.tier))?)
    } else {
        eprintln!(
            "· no cleanup model for the {} tier; using rules only (muskflow models install {})",
            c.tier, c.tier
        );
        None
    };
    let events = mf_hotkey::start()?;
    // macOS lets a tap start without Input Monitoring but then never delivers a key to it,
    // so "ready" would be a lie. The permission belongs to the app MuskFlow runs inside.
    if !mf_hotkey::has_permission() {
        anyhow::bail!(
            "Fn won't reach MuskFlow until the app it runs in has Input Monitoring. Turn it on in System Settings → Privacy & Security → Input Monitoring, quit and reopen that app, then run this again.\n  Tip: use macOS Terminal, since apps that host a terminal (like Claude) need a full restart after the change."
        );
    }
    eprintln!(
        "· ready ({} tier). Hold Fn and speak; let go to type it. Fn + Control: command mode. Ctrl+C quits.",
        c.tier
    );

    loop {
        let Event::Down(mode) = events.recv()? else {
            continue;
        };
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
                continue;
            }
            Err(e) => {
                eprintln!("→ {e:#}");
                continue;
            }
        };
        let spoke = samples_to_secs(rec.audio.len());
        if spoke < MIN_SPEECH_SECS {
            eprintln!("→ too short");
            continue;
        }
        if let Some(problem) = rec.problem(&vad) {
            eprintln!("→ {problem}");
            continue;
        }
        if final_mode == Mode::Command {
            eprint!("(command mode isn't built yet; typing it as dictation) ");
        }

        let raw = rec.hyp.text();
        let t = Instant::now();
        let text = match &cleaner {
            Some(cl) => match cl.clean(&raw) {
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
            continue;
        }
        // A trailing space so the next dictation doesn't run into this one.
        match mf_insert::insert(&format!("{text} ")) {
            Ok(()) => eprintln!(
                "→ typed in {} (speech {} + cleanup {}) · spoke {spoke:.1}s\n  {text}",
                ms(rec.stopped.elapsed()),
                ms(rec.finish),
                ms(clean_time)
            ),
            Err(e) => eprintln!("→ {e:#}\n  {text}"),
        }
    }
}

/// Asks for both permissions up front, so the macOS prompts appear at start, not mid-dictation.
fn check_permissions() {
    if !mf_hotkey::has_permission() {
        mf_hotkey::request_permission();
        eprintln!(
            "· MuskFlow needs Input Monitoring to see the Fn key: System Settings → Privacy & Security → Input Monitoring. Turn on the app you run MuskFlow from (Terminal, iTerm, Claude…), then restart it."
        );
    }
    if !mf_insert::has_permission() {
        mf_insert::request_permission();
        eprintln!(
            "· MuskFlow needs Accessibility to type into other apps: System Settings → Privacy & Security → Accessibility. Turn on the app you run MuskFlow from."
        );
    }
    eprintln!(
        "· tip: set System Settings → Keyboard → \"Press 🌐 key to\" → Do Nothing, so Fn doesn't also open the emoji picker."
    );
}
