//! The user's settings: ages, quick categories and the hotkey mode,
//! background analysis. Kept in the index as a note, one `key=value`
//! line each, so a newer or older engine reads what it understands.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::domain::{Category, QuickCategory, Recency};
use crate::git::WorktreeSettings;

/// What the cleaning hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyMode {
    /// Show what was found and ask to confirm.
    Confirm,
    /// Clean the quick categories at once.
    Immediate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Watch the disk and keep the picture fresh.
    pub background: bool,
    /// Use the Saver profile on battery (the app decides; kept here).
    pub reduce_on_battery: bool,
    /// Build output active within this age is not preselected.
    pub recent_age: Duration,
    /// A worktree changed within this age is in use.
    pub stale_after: Duration,
    pub hotkey: HotkeyMode,
    /// What the hotkey cleans in `Immediate` mode.
    pub quick: BTreeSet<QuickCategory>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            background: true,
            reduce_on_battery: true,
            recent_age: Recency::DEFAULT_AGE,
            stale_after: WorktreeSettings::DEFAULT_STALE_AFTER,
            hotkey: HotkeyMode::Confirm,
            quick: BTreeSet::new(),
        }
    }
}

const QUICK: [QuickCategory; 3] = [
    QuickCategory::AppCache,
    QuickCategory::DevCache,
    QuickCategory::BuildArtifact,
];

fn quick_name(quick: QuickCategory) -> &'static str {
    let category = match quick {
        QuickCategory::AppCache => Category::AppCache,
        QuickCategory::DevCache => Category::DevCache,
        QuickCategory::BuildArtifact => Category::BuildArtifact,
    };
    category.name()
}

impl Settings {
    /// As stored: one `key=value` line per setting.
    pub fn encode(&self) -> String {
        let quick: Vec<&str> = self.quick.iter().map(|q| quick_name(*q)).collect();
        let hotkey = match self.hotkey {
            HotkeyMode::Confirm => "confirm",
            HotkeyMode::Immediate => "immediate",
        };
        format!(
            "background={}\nreduce_on_battery={}\nrecent_age={}\nstale_after={}\nhotkey={hotkey}\nquick={}\n",
            u8::from(self.background),
            u8::from(self.reduce_on_battery),
            self.recent_age.as_secs(),
            self.stale_after.as_secs(),
            quick.join(","),
        )
    }

    /// Settings read back; what is missing or not understood keeps its
    /// default.
    pub fn decode(text: &str) -> Settings {
        let mut settings = Settings::default();
        for (key, value) in text.lines().filter_map(|line| line.split_once('=')) {
            settings.read(key, value);
        }
        settings
    }

    fn read(&mut self, key: &str, value: &str) {
        let seconds = || value.parse().ok().map(Duration::from_secs);
        match key {
            "background" => self.background = value != "0",
            "reduce_on_battery" => self.reduce_on_battery = value != "0",
            "recent_age" => self.recent_age = seconds().unwrap_or(self.recent_age),
            "stale_after" => self.stale_after = seconds().unwrap_or(self.stale_after),
            "hotkey" => self.hotkey = hotkey(value).unwrap_or(self.hotkey),
            "quick" => self.quick = quick(value),
            _ => {}
        }
    }
}

fn hotkey(value: &str) -> Option<HotkeyMode> {
    match value {
        "confirm" => Some(HotkeyMode::Confirm),
        "immediate" => Some(HotkeyMode::Immediate),
        _ => None,
    }
}

fn quick(value: &str) -> BTreeSet<QuickCategory> {
    let names: BTreeSet<&str> = value.split(',').collect();
    QUICK
        .into_iter()
        .filter(|q| names.contains(quick_name(*q)))
        .collect()
}
