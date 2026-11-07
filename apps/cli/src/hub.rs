//! The Umless window's backend: answers the page's requests (history, stats,
//! dictionary, settings). Runs on the main thread; every call is a quick file read
//! or write.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use umless_models::Tier;

use crate::history::{Entry, History};
use crate::settings::Settings;

/// Typing speed used for "time saved": a typical typist, not a fast one.
const TYPING_WPM: f64 = 40.0;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

pub struct Hub {
    pub history: PathBuf,
    pub dictionary: PathBuf,
    pub tier: Tier,
    pub models: PathBuf,
    pub settings: Settings,
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    cmd: String,
    #[serde(default)]
    args: Value,
}

impl Hub {
    pub fn into_backend(self) -> umless_shell::Backend {
        Box::new(move |req: &str| {
            let (id, result) = match serde_json::from_str::<Request>(req) {
                Ok(r) => (r.id, self.handle(&r.cmd, &r.args)),
                Err(e) => (0, Err(anyhow::anyhow!("bad request: {e}"))),
            };
            match result {
                Ok(v) => json!({ "id": id, "result": v }),
                Err(e) => json!({ "id": id, "error": format!("{e:#}") }),
            }
            .to_string()
        })
    }

    fn handle(&self, cmd: &str, args: &Value) -> Result<Value> {
        let history = History::new(&self.history);
        Ok(match cmd {
            "home" => {
                let entries = history.load();
                json!({ "stats": stats(&entries), "recent": entries.iter().take(5).collect::<Vec<_>>() })
            }
            "history" => json!(history.load()),
            "delete" => {
                history.delete(Some(arg_u64(args, "at")?))?;
                json!(true)
            }
            "clear" => {
                history.delete(None)?;
                json!(true)
            }
            "copy" => {
                umless_insert::copy(arg_str(args, "text")?)?;
                json!(true)
            }
            "dictionary" => json!(self.terms()?),
            "dict_add" => {
                let term = arg_str(args, "term")?.trim();
                if term.is_empty() {
                    bail!("type a word or name first");
                }
                if !self.terms()?.iter().any(|t| t.eq_ignore_ascii_case(term)) {
                    let mut text = std::fs::read_to_string(&self.dictionary).unwrap_or_default();
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(term);
                    text.push('\n');
                    std::fs::write(&self.dictionary, text)?;
                }
                json!(self.terms()?)
            }
            "dict_remove" => {
                let term = arg_str(args, "term")?;
                let text = std::fs::read_to_string(&self.dictionary)?;
                let kept: Vec<&str> = text
                    .lines()
                    .filter(|l| line_term(l) != Some(term))
                    .collect();
                std::fs::write(&self.dictionary, kept.join("\n") + "\n")?;
                json!(self.terms()?)
            }
            "settings" => json!({
                "version": env!("CARGO_PKG_VERSION"),
                "tier": self.tier.to_string(),
                "speech": self.tier.asr_label(),
                "cleanup": self.tier.llm_label_in(&self.models),
                "login": umless_shell::launch_at_login(),
                "screen_context": self.settings.screen_context(),
                "learning": self.settings.learning(),
                "input_monitoring": umless_hotkey::has_permission(),
                "accessibility": umless_insert::has_permission(),
                "dictionary": self.dictionary.display().to_string(),
                "history": self.history.display().to_string(),
            }),
            "style" => json!({
                "current": self.settings.style().map_or("auto", |s| s.name()),
                "lite": self.tier == Tier::Lite,
            }),
            "set_style" => {
                let style = match arg_str(args, "style")? {
                    "auto" => None,
                    name => Some(name.parse::<umless_cleanup::Style>()?),
                };
                self.settings.set_style(style)?;
                json!(true)
            }
            "setup" => setup_status(),
            "tiers" => json!(
                Tier::ALL
                    .iter()
                    .map(|t| json!({
                        "name": t.to_string(),
                        "speech": t.asr_label(),
                        "cleanup": t.llm_label_in(&self.models),
                        "installed": t.is_installed(&self.models),
                        "current": *t == self.tier,
                    }))
                    .collect::<Vec<_>>()
            ),
            "set_tier" => {
                let tier: Tier = arg_str(args, "tier")?.parse()?;
                if !tier.is_installed(&self.models) {
                    bail!(
                        "{tier} isn't downloaded yet. Run ./install.sh {tier} in the Umless folder."
                    );
                }
                self.settings.set_tier(tier)?;
                restart();
                json!(true)
            }
            "request" => {
                match arg_str(args, "what")? {
                    "mic" => umless_audio::request_mic_permission(),
                    "accessibility" => {
                        umless_insert::request_permission();
                        open_settings("Privacy_Accessibility");
                    }
                    "input_monitoring" => {
                        umless_hotkey::request_permission();
                        open_settings("Privacy_ListenEvent");
                    }
                    "mic_settings" => open_settings("Privacy_Microphone"),
                    "keyboard" => {
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open")
                            .arg("x-apple.systempreferences:com.apple.Keyboard-Settings.extension")
                            .spawn();
                    }
                    other => bail!("can't request {other}"),
                }
                json!(true)
            }
            // Clears a permission entry left from an older build (a different signature), so
            // macOS asks afresh. Only touches Umless's own entry.
            "reset_permission" => {
                let service = match arg_str(args, "what")? {
                    "accessibility" => "Accessibility",
                    "input_monitoring" => "ListenEvent",
                    other => bail!("can't reset {other}"),
                };
                #[cfg(target_os = "macos")]
                std::process::Command::new("tccutil")
                    .args(["reset", service, BUNDLE_ID])
                    .status()
                    .context("could not run tccutil")?;
                #[cfg(not(target_os = "macos"))]
                let _ = (service, BUNDLE_ID);
                json!(true)
            }
            "restart" => {
                restart();
                json!(true)
            }
            "set_screen_context" => {
                self.settings
                    .set_screen_context(args["on"].as_bool().unwrap_or(true))?;
                json!(self.settings.screen_context())
            }
            "set_learning" => {
                self.settings
                    .set_learning(args["on"].as_bool().unwrap_or(true))?;
                json!(self.settings.learning())
            }
            "set_login" => {
                umless_shell::set_launch_at_login(args["on"].as_bool().unwrap_or(false))?;
                json!(umless_shell::launch_at_login())
            }
            "open" => {
                use crate::paths::Open;
                let (how, path) = match arg_str(args, "what")? {
                    "dictionary" => (Open::Edit, self.dictionary.clone()),
                    "history" => (Open::Reveal, self.history.clone()),
                    "log" => (Open::Edit, log_path()),
                    "privacy" => (Open::Default, PathBuf::from(PRIVACY_SETTINGS)),
                    other => bail!("can't open {other}"),
                };
                crate::paths::open(&path, how).context("could not open it")?;
                json!(true)
            }
            other => bail!("unknown command {other}"),
        })
    }

    /// The dictionary's terms in file order (comments and blank lines skipped).
    fn terms(&self) -> Result<Vec<String>> {
        let text = std::fs::read_to_string(&self.dictionary).unwrap_or_default();
        Ok(text
            .lines()
            .filter_map(line_term)
            .map(String::from)
            .collect())
    }
}

const BUNDLE_ID: &str = "io.github.jatinbishla.umless";

#[cfg(target_os = "macos")]
const PRIVACY_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";
#[cfg(windows)]
const PRIVACY_SETTINGS: &str = "ms-settings:privacy-microphone";
#[cfg(not(any(target_os = "macos", windows)))]
const PRIVACY_SETTINGS: &str = "settings://";

fn open_settings(pane: &str) {
    if let Some(url) = settings_url(pane) {
        let _ = crate::paths::open(std::path::Path::new(&url), crate::paths::Open::Default);
    }
}

/// The OS settings page for a macOS privacy pane name. Windows has one only for the
/// microphone; Linux has none (permissions are groups and installed tools).
fn settings_url(pane: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        Some(format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ))
    } else if cfg!(windows) && pane == "Privacy_Microphone" {
        Some("ms-settings:privacy-microphone".into())
    } else {
        None
    }
}

/// Everything the setup screen checks. `done` once dictation can work end to end.
fn setup_status() -> Value {
    let mic = match umless_audio::mic_permission() {
        umless_audio::MicPermission::Granted => "granted",
        umless_audio::MicPermission::Denied => "denied",
        umless_audio::MicPermission::NotAsked => "not_asked",
    };
    let accessibility = umless_insert::has_permission();
    let input_monitoring = umless_hotkey::has_permission();
    let fn_key = fn_key_action();
    json!({
        "mic": mic,
        "accessibility": accessibility,
        "input_monitoring": input_monitoring,
        "fn_key": fn_key,
        "done": mic == "granted" && accessibility && input_monitoring,
    })
}

/// What the 🌐/fn key does on its own (System Settings → Keyboard → "Press 🌐 key to").
fn fn_key_action() -> &'static str {
    // Only macOS has a 🌐 key setting; elsewhere the hold key has no system action.
    if !cfg!(target_os = "macos") {
        return "nothing";
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "com.apple.HIToolbox", "AppleFnUsageType"])
        .output();
    match out
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .as_deref()
    {
        Some("0") => "nothing",
        Some("1") => "input_source",
        Some("2") => "emoji",
        Some("3") => "dictation",
        // Never changed: macOS's default is the emoji picker.
        _ => "emoji",
    }
}

/// Starts a fresh copy of Umless and quits this one. Input Monitoring only takes effect
/// in a process started after it was allowed.
fn restart() {
    let bundle = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(PathBuf::from)
    });
    if let Some(app) = bundle {
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("sleep 0.5; open -n {:?}", app))
            .spawn();
        std::process::exit(0);
    }
    // Not a macOS bundle (Windows, Linux, or a terminal build): start the same binary again.
    #[cfg(not(target_os = "macos"))]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .spawn();
        std::process::exit(0);
    }
}

fn line_term(line: &str) -> Option<&str> {
    let t = line.split('#').next().unwrap_or("").trim();
    (!t.is_empty()).then_some(t)
}

fn log_path() -> PathBuf {
    crate::paths::log_path()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key].as_str().with_context(|| format!("missing {key}"))
}

fn arg_u64(args: &Value, key: &str) -> Result<u64> {
    args[key].as_u64().with_context(|| format!("missing {key}"))
}

fn stats(entries: &[Entry]) -> Value {
    let now = History::now_ms();
    let words = |e: &[&Entry]| e.iter().map(|x| x.words()).sum::<usize>();
    let all: Vec<&Entry> = entries.iter().collect();
    let week: Vec<&Entry> = entries.iter().filter(|e| now - e.at < 7 * DAY_MS).collect();
    let today: Vec<&Entry> = entries.iter().filter(|e| now - e.at < DAY_MS).collect();
    let spoke: f64 = entries.iter().map(|e| e.spoke_secs as f64).sum();
    let total_words = words(&all);
    // Minutes saved: typing it at TYPING_WPM, minus the time actually spent speaking.
    let saved_min = (total_words as f64 / TYPING_WPM - spoke / 60.0).max(0.0);
    let wpm = if spoke > 0.0 {
        total_words as f64 / (spoke / 60.0)
    } else {
        0.0
    };
    let mut latencies: Vec<u32> = entries.iter().map(|e| e.latency_ms).collect();
    latencies.sort_unstable();
    let median = latencies.get(latencies.len() / 2).copied().unwrap_or(0);
    json!({
        "words": total_words,
        "words_week": words(&week),
        "words_today": words(&today),
        "dictations": entries.len(),
        "wpm": wpm.round(),
        "saved_min": saved_min.round(),
        "latency_ms": median,
    })
}
