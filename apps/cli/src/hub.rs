//! The MuskFlow window's backend: answers the page's requests (history, stats,
//! dictionary, settings). Runs on the main thread; every call is a quick file read
//! or write.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use mf_models::Tier;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::history::{Entry, History};

/// Typing speed used for "time saved": a typical typist, not a fast one.
const TYPING_WPM: f64 = 40.0;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

pub struct Hub {
    pub history: PathBuf,
    pub dictionary: PathBuf,
    pub tier: Tier,
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    cmd: String,
    #[serde(default)]
    args: Value,
}

impl Hub {
    pub fn into_backend(self) -> mf_shell::Backend {
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
                mf_insert::copy(arg_str(args, "text")?)?;
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
                "cleanup": self.tier.llm_label(),
                "login": mf_shell::launch_at_login(),
                "input_monitoring": mf_hotkey::has_permission(),
                "accessibility": mf_insert::has_permission(),
                "dictionary": self.dictionary.display().to_string(),
                "history": self.history.display().to_string(),
            }),
            "set_login" => {
                mf_shell::set_launch_at_login(args["on"].as_bool().unwrap_or(false))?;
                json!(mf_shell::launch_at_login())
            }
            "open" => {
                let (flag, path) = match arg_str(args, "what")? {
                    "dictionary" => ("-t", self.dictionary.clone()),
                    "history" => ("-R", self.history.clone()),
                    "log" => ("-t", log_path()),
                    "privacy" => (
                        "",
                        PathBuf::from(
                            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
                        ),
                    ),
                    other => bail!("can't open {other}"),
                };
                let mut open = std::process::Command::new("open");
                if !flag.is_empty() {
                    open.arg(flag);
                }
                open.arg(path).spawn().context("could not run open")?;
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

fn line_term(line: &str) -> Option<&str> {
    let t = line.split('#').next().unwrap_or("").trim();
    (!t.is_empty()).then_some(t)
}

fn log_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Logs/MuskFlow.log")
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
