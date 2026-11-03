//! The user's choices from the MuskFlow window, in
//! `~/Library/Application Support/MuskFlow/settings.json`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mf_cleanup::Style;
use mf_models::Tier;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Data {
    /// A fixed style, or `None` to pick one from the app being typed into.
    #[serde(default)]
    style: Option<String>,
    /// Model tier picked in the window; overrides the one the app was built with.
    #[serde(default)]
    tier: Option<String>,
}

/// Shared between the dictation loop and the window; saved on every change.
#[derive(Clone)]
pub struct Settings {
    path: PathBuf,
    data: Arc<Mutex<Data>>,
}

impl Settings {
    pub fn load() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let path = home.join("Library/Application Support/MuskFlow/settings.json");
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Arc::new(Mutex::new(data)),
        }
    }

    /// The style the user fixed, or `None` for automatic.
    pub fn style(&self) -> Option<Style> {
        self.data.lock().unwrap().style.as_deref()?.parse().ok()
    }

    pub fn set_style(&self, style: Option<Style>) -> anyhow::Result<()> {
        self.update(|d| d.style = style.map(|s| s.name().to_string()))
    }

    pub fn tier(&self) -> Option<Tier> {
        self.data.lock().unwrap().tier.as_deref()?.parse().ok()
    }

    pub fn set_tier(&self, tier: Tier) -> anyhow::Result<()> {
        self.update(|d| d.tier = Some(tier.to_string()))
    }

    fn update(&self, change: impl FnOnce(&mut Data)) -> anyhow::Result<()> {
        let mut d = self.data.lock().unwrap();
        change(&mut d);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&*d)?)?;
        Ok(())
    }
}
