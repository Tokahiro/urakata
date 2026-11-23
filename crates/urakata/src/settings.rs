//! Settings that the watcher uses, in `%LOCALAPPDATA%\urakata\settings.json`.
//! The settings window writes the file; the watcher reads it again each
//! time it reads the monitor list, so a change applies to the next stream.
//! A missing file means the defaults, the values used in the host tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::layout::{data_dir, write_json};

/// Limits of the values. A file with a value out of range is not an
/// error: the value is clamped.
pub const MAX_START_DELAY_S: u32 = 30;
pub const MAX_GUARD_S: u32 = 120;
pub const MAX_GUARD_REAPPLY: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Turn the other displays off when a stream starts. Off: the watcher
    /// only cleans up a saved layout after a stream.
    pub exclusive: bool,
    /// Seconds from the new SudoVDA monitor to the first `exclusive`.
    pub start_delay_s: u32,
    /// Seconds after `exclusive` in which the guard turns displays off
    /// again when another program turned them on.
    pub guard_s: u32,
    /// How often the guard may turn the displays off again.
    pub guard_reapply: u32,
    /// `urakata-watch.exe` shows its icon in the notification area.
    pub tray_icon: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // Steam sets its stream layout about 4 s after the monitor arrives
        // (measured on a test host).
        Self {
            exclusive: true,
            start_delay_s: 3,
            guard_s: 20,
            guard_reapply: 3,
            tray_icon: true,
        }
    }
}

impl Settings {
    /// The same settings with each value in its range.
    pub fn clamped(self) -> Self {
        Self {
            start_delay_s: self.start_delay_s.min(MAX_START_DELAY_S),
            guard_s: self.guard_s.min(MAX_GUARD_S),
            guard_reapply: self.guard_reapply.min(MAX_GUARD_REAPPLY),
            ..self
        }
    }

    pub fn start_delay(&self) -> Duration {
        Duration::from_secs(self.start_delay_s.into())
    }

    pub fn guard(&self) -> Duration {
        Duration::from_secs(self.guard_s.into())
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let mut line = if self.exclusive {
            format!(
                "Settings: exclusive on, start delay {} s, guard {} s, guard re-applies {}.",
                self.start_delay_s, self.guard_s, self.guard_reapply
            )
        } else {
            "Settings: exclusive off.".into()
        };
        if !self.tray_icon {
            line.push_str(" No tray icon.");
        }
        line
    }
}

/// `%LOCALAPPDATA%\urakata\settings.json`.
pub fn settings_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("settings.json"))
}

/// Reads the settings. A missing file gives the defaults.
pub fn load(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Result<Settings, String> {
    serde_json::from_str::<Settings>(text)
        .map(Settings::clamped)
        .map_err(|e| e.to_string())
}

/// Writes the settings through a temporary file and a rename, so that the
/// watcher never reads half a file.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    write_json(path, &settings.clamped())
}

/// Turns repeated file reads into the settings to use and a log line only
/// when the result changed, so that the watcher does not log the same
/// settings at each 60 s read.
#[derive(Debug, Default)]
pub struct Tracker {
    last: Option<Result<Settings, String>>,
}

impl Tracker {
    /// The settings to use for a read result (a damaged file gives the
    /// defaults) and a line for the log when the result changed.
    pub fn update(&mut self, read: Result<Settings, String>) -> (Settings, Option<String>) {
        let line = (self.last.as_ref() != Some(&read)).then(|| match &read {
            Ok(s) => s.summary(),
            Err(e) => format!("Cannot use the settings ({e}). Using the defaults."),
        });
        let settings = read.clone().unwrap_or_default();
        self.last = Some(read);
        (settings, line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_values_take_defaults() {
        let s = parse(r#"{"exclusive": false}"#).unwrap();
        assert_eq!(
            s,
            Settings {
                exclusive: false,
                ..Settings::default()
            }
        );
        assert_eq!(parse("{}").unwrap(), Settings::default());
        // A file from before the tray icon keeps the icon on.
        assert!(
            parse(r#"{"exclusive": true, "guard_s": 5}"#)
                .unwrap()
                .tray_icon
        );
    }

    #[test]
    fn values_are_clamped() {
        let s = parse(r#"{"start_delay_s": 999, "guard_s": 999, "guard_reapply": 99}"#).unwrap();
        assert_eq!(s.start_delay_s, MAX_START_DELAY_S);
        assert_eq!(s.guard_s, MAX_GUARD_S);
        assert_eq!(s.guard_reapply, MAX_GUARD_REAPPLY);
    }

    #[test]
    fn damaged_file_is_an_error() {
        assert!(parse("{").is_err());
        assert!(parse(r#"{"guard_s": -1}"#).is_err());
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("urakata-settings-{}", std::process::id()));
        // A failed earlier run can leave the folder.
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert_eq!(load(&path).unwrap(), Settings::default());
        let s = Settings {
            exclusive: false,
            start_delay_s: 5,
            guard_s: 30,
            guard_reapply: 1,
            tray_icon: false,
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        // A second save replaces the file.
        let t = Settings { guard_s: 999, ..s };
        save(&path, &t).unwrap();
        assert_eq!(load(&path).unwrap().guard_s, MAX_GUARD_S);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tracker_logs_only_changes_and_falls_back() {
        let mut t = Tracker::default();
        let (s, line) = t.update(Ok(Settings::default()));
        assert_eq!(s, Settings::default());
        assert_eq!(
            line.as_deref(),
            Some(Settings::default().summary().as_str())
        );
        assert_eq!(t.update(Ok(Settings::default())).1, None);
        let off = Settings {
            exclusive: false,
            ..Settings::default()
        };
        assert_eq!(
            t.update(Ok(off)),
            (off, Some("Settings: exclusive off.".into()))
        );
        let hidden = Settings {
            tray_icon: false,
            ..off
        };
        assert_eq!(
            t.update(Ok(hidden)).1.as_deref(),
            Some("Settings: exclusive off. No tray icon.")
        );
        let (s, line) = t.update(Err("bad".into()));
        assert_eq!(s, Settings::default());
        assert_eq!(
            line.as_deref(),
            Some("Cannot use the settings (bad). Using the defaults.")
        );
        assert_eq!(t.update(Err("bad".into())), (Settings::default(), None));
    }
}
