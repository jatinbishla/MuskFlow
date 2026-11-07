//! The user's choices from the Umless window, in
//! `settings.json` in the data folder (`paths::data_dir`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use umless_cleanup::Style;
use umless_models::Tier;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Data {
    /// A fixed style, or `None` to pick one from the app being typed into.
    #[serde(default)]
    style: Option<String>,
    /// Model tier picked in the window; overrides the one the app was built with.
    #[serde(default)]
    tier: Option<String>,
    /// Read the text on screen (names, tone, command-mode context). On unless turned off.
    #[serde(default)]
    no_screen_context: bool,
    /// Add words the user corrects after a dictation to the dictionary. On unless turned off.
    #[serde(default)]
    no_learning: bool,
}

/// Shared between the dictation loop and the window; saved on every change.
#[derive(Clone)]
pub struct Settings {
    path: PathBuf,
    data: Arc<Mutex<Data>>,
}

impl Settings {
    pub fn load() -> Self {
        let path = crate::paths::data_dir().join("settings.json");
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

    pub fn screen_context(&self) -> bool {
        !self.data.lock().unwrap().no_screen_context
    }

    pub fn set_screen_context(&self, on: bool) -> anyhow::Result<()> {
        self.update(|d| d.no_screen_context = !on)
    }

    pub fn learning(&self) -> bool {
        !self.data.lock().unwrap().no_learning
    }

    pub fn set_learning(&self, on: bool) -> anyhow::Result<()> {
        self.update(|d| d.no_learning = !on)
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
