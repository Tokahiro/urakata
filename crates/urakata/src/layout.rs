//! Rules for `exclusive` and `restore` that need no Windows API, and the
//! session file. The Windows calls are in `display.rs`.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::report::Display;

/// `DISPLAYCONFIG_PATH_MODE_IDX_INVALID`: the path refers to no mode.
pub const NO_MODE: u32 = u32::MAX;

/// Session file format version.
const SESSION_VERSION: u32 = 1;

/// The layout before `exclusive` changed it. While this file exists, a
/// session is active: `restore` (or a later repair at logon) must run.
#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    /// File format version.
    pub version: u32,
    /// Seconds since the Unix epoch when the layout was saved.
    pub saved_at_unix: u64,
    /// Active displays at that time.
    pub displays: Vec<Display>,
}

impl Session {
    /// A session for the given displays, saved now.
    pub fn now(displays: Vec<Display>) -> Self {
        Self {
            version: SESSION_VERSION,
            saved_at_unix: now_unix(),
            displays,
        }
    }

    /// When the layout was saved, as an age such as "5 min ago".
    pub fn age(&self) -> String {
        age_text(self.saved_at_unix, now_unix())
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The age of a time stamp in words: "just now", "5 min ago", "3 h ago",
/// "2 days ago". A stamp in the future (clock changed) is "just now".
pub fn age_text(then_unix: u64, now_unix: u64) -> String {
    let s = now_unix.saturating_sub(then_unix);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "1 day ago".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// `%LOCALAPPDATA%\urakata`: saved layout, settings and watcher log.
pub fn data_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(base).join("urakata"))
}

/// `%LOCALAPPDATA%\urakata\session.json`.
pub fn session_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("session.json"))
}

/// Why the session file could not be loaded.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Reading failed; this can pass (for example a scan holds the file).
    Read(String),
    /// The file was read but is damaged or of an unknown version.
    Damaged(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) | Self::Damaged(e) => f.write_str(e),
        }
    }
}

/// Reads the session file. `None` when it does not exist.
pub fn load_session(path: &Path) -> Result<Option<Session>, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LoadError::Read(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let session: Session = serde_json::from_str(&text)
        .map_err(|e| LoadError::Damaged(format!("cannot parse {}: {e}", path.display())))?;
    if session.version != SESSION_VERSION {
        return Err(LoadError::Damaged(format!(
            "{}: unknown version {}",
            path.display(),
            session.version
        )));
    }
    Ok(Some(session))
}

/// Writes the session file through a temporary file and a rename, so that
/// a crash leaves either the old file or the whole new one.
pub fn save_session(path: &Path, session: &Session) -> Result<(), String> {
    write_json(path, session)
}

/// Writes `value` as JSON through a temporary file and a rename, so that
/// a reader sees either the old file or the whole new one.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Deletes the session file. A missing file is not an error.
pub fn remove_session(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot delete {}: {e}", path.display())),
    }
}

/// Mode indexes of the kept paths, renumbered for a mode list that holds
/// only the modes these paths use.
#[derive(Debug, PartialEq, Eq)]
pub struct Remap {
    /// New (source, target) mode index per kept path.
    pub refs: Vec<(u32, u32)>,
    /// Old index of each mode in the new list.
    pub modes: Vec<usize>,
}

/// Renumbers the (source, target) mode indexes of the kept paths. A mode
/// that two paths share (clone) stays one mode. `NO_MODE` stays `NO_MODE`.
pub fn remap_modes(refs: &[(u32, u32)], mode_count: usize) -> Result<Remap, String> {
    let mut new_of_old: Vec<Option<u32>> = vec![None; mode_count];
    let mut out = Remap {
        refs: Vec::with_capacity(refs.len()),
        modes: Vec::new(),
    };
    let mut map = |old: u32| -> Result<u32, String> {
        if old == NO_MODE {
            return Ok(NO_MODE);
        }
        let slot = new_of_old
            .get_mut(old as usize)
            .ok_or_else(|| format!("mode index {old} out of range ({mode_count} modes)"))?;
        Ok(*slot.get_or_insert_with(|| {
            out.modes.push(old as usize);
            (out.modes.len() - 1) as u32
        }))
    };
    let mut new_refs = Vec::with_capacity(refs.len());
    for &(source, target) in refs {
        new_refs.push((map(source)?, map(target)?));
    }
    out.refs = new_refs;
    Ok(out)
}

/// Offset that moves the kept sources so that one is at (0, 0), the
/// primary position. No offset when one is there already.
pub fn primary_offset(positions: &[(i32, i32)]) -> (i32, i32) {
    match positions.first() {
        Some(_) if positions.contains(&(0, 0)) => (0, 0),
        Some(&(x, y)) => (-x, -y),
        None => (0, 0),
    }
}

fn same_monitor(a: &Display, b: &Display) -> bool {
    !a.monitor_path.is_empty() && a.monitor_path.eq_ignore_ascii_case(&b.monitor_path)
}

/// Saved physical displays that are not active now.
pub fn missing_displays<'a>(saved: &'a [Display], current: &[Display]) -> Vec<&'a Display> {
    saved
        .iter()
        .filter(|s| !s.sudovda && !current.iter().any(|c| same_monitor(s, c)))
        .collect()
}

/// Displays that are active now at another position than saved, as
/// (saved, current). Only checked while a saved SudoVDA display is still
/// active: then the stream that `exclusive` changed still runs, and the
/// saved positions are the ones to return to. After the stream, Steam's
/// layout is gone and the positions differ on purpose.
pub fn moved_displays<'a, 'b>(
    saved: &'a [Display],
    current: &'b [Display],
) -> Vec<(&'a Display, &'b Display)> {
    let same_stream = saved
        .iter()
        .any(|s| s.sudovda && current.iter().any(|c| same_monitor(s, c)));
    if !same_stream {
        return Vec::new();
    }
    saved
        .iter()
        .filter_map(|s| current.iter().find(|c| same_monitor(s, c)).map(|c| (s, c)))
        .filter(|(s, c)| (s.x, s.y) != (c.x, c.y))
        .collect()
}

/// For each current display whose HDR (advanced color) state differs from
/// the saved one: its index in `current` and the saved state. Only
/// physical displays that support advanced color count.
pub fn hdr_fixes(saved: &[Display], current: &[Display]) -> Vec<(usize, bool)> {
    let mut fixes = Vec::new();
    for (i, c) in current.iter().enumerate() {
        // With "wide color enforced" (automatic color management on an SDR
        // display), `enabled` does not mean HDR; leave such displays alone.
        let hdr_capable = |a: &&crate::report::AdvancedColor| a.supported && !a.wide_color_enforced;
        let Some(now) = c.advanced_color.as_ref().filter(hdr_capable) else {
            continue;
        };
        let before = saved
            .iter()
            .filter(|s| !s.sudovda && same_monitor(s, c))
            .find_map(|s| s.advanced_color.as_ref().filter(hdr_capable));
        if let Some(before) = before.filter(|b| b.enabled != now.enabled) {
            fixes.push((i, before.enabled));
        }
    }
    fixes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::AdvancedColor;

    #[test]
    fn ages_in_words() {
        let t = 1_795_379_667;
        assert_eq!(age_text(t, t), "just now");
        assert_eq!(age_text(t, t + 59), "just now");
        assert_eq!(age_text(t + 10, t), "just now");
        assert_eq!(age_text(t, t + 60), "1 min ago");
        assert_eq!(age_text(t, t + 3599), "59 min ago");
        assert_eq!(age_text(t, t + 3600), "1 h ago");
        assert_eq!(age_text(t, t + 86_399), "23 h ago");
        assert_eq!(age_text(t, t + 86_400), "1 day ago");
        assert_eq!(age_text(t, t + 3 * 86_400), "3 days ago");
    }

    #[test]
    fn remap_keeps_only_used_modes() {
        // Modes 0..6; keep the paths that use (4, 5) and (2, 3).
        let r = remap_modes(&[(4, 5), (2, 3)], 6).unwrap();
        assert_eq!(r.refs, vec![(0, 1), (2, 3)]);
        assert_eq!(r.modes, vec![4, 5, 2, 3]);
    }

    #[test]
    fn remap_shares_clone_source_and_keeps_invalid() {
        let r = remap_modes(&[(1, NO_MODE), (1, 0)], 2).unwrap();
        assert_eq!(r.refs, vec![(0, NO_MODE), (0, 1)]);
        assert_eq!(r.modes, vec![1, 0]);
    }

    #[test]
    fn remap_rejects_bad_index() {
        assert!(remap_modes(&[(0, 7)], 2).is_err());
    }

    #[test]
    fn offset_moves_first_source_to_origin() {
        assert_eq!(primary_offset(&[(0, 0)]), (0, 0));
        assert_eq!(primary_offset(&[(2560, 0), (0, 0)]), (0, 0));
        assert_eq!(primary_offset(&[(-2560, 120)]), (2560, -120));
        assert_eq!(primary_offset(&[]), (0, 0));
    }

    fn display(path: &str, sudovda: bool, hdr: Option<(bool, bool)>) -> Display {
        Display {
            monitor_path: path.into(),
            sudovda,
            advanced_color: hdr.map(|(supported, enabled)| AdvancedColor {
                supported,
                enabled,
                bits_per_color_channel: 8,
                wide_color_enforced: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn missing_ignores_virtual_displays() {
        let saved = [
            display("A", false, None),
            display("B", false, None),
            display("V", true, None),
        ];
        let current = [display("a", false, None)];
        let missing: Vec<_> = missing_displays(&saved, &current)
            .iter()
            .map(|d| d.monitor_path.as_str())
            .collect();
        assert_eq!(missing, vec!["B"]);
    }

    #[test]
    fn hdr_fixes_only_changed_physical_displays() {
        let saved = [
            display("LG", false, Some((true, true))),
            display("P1", false, Some((false, false))),
            display("P2", false, Some((true, false))),
            display("V", true, Some((true, false))),
        ];
        let current = [
            display("P2", false, Some((true, false))),
            display("LG", false, Some((true, false))),
            display("P1", false, Some((false, false))),
            display("V", true, Some((true, true))),
            display("NEW", false, Some((true, true))),
        ];
        assert_eq!(hdr_fixes(&saved, &current), vec![(1, true)]);
    }

    #[test]
    fn hdr_fixes_skip_wide_color_enforced() {
        let saved = [display("P", false, Some((true, true)))];
        let mut now = display("P", false, Some((true, false)));
        now.advanced_color.as_mut().unwrap().wide_color_enforced = true;
        assert!(hdr_fixes(&saved, &[now]).is_empty());
    }

    fn at(path: &str, sudovda: bool, x: i32) -> Display {
        Display {
            x,
            ..display(path, sudovda, None)
        }
    }

    #[test]
    fn moved_only_while_the_saved_stream_runs() {
        let saved = [at("V", true, 0), at("LG", false, -2560)];
        let moved = [at("V", true, 2560), at("LG", false, 0)];
        let found: Vec<_> = moved_displays(&saved, &moved)
            .iter()
            .map(|(s, _)| s.monitor_path.as_str())
            .collect();
        assert_eq!(found, vec!["V", "LG"]);
        assert!(moved_displays(&saved, &[at("V", true, 0), at("LG", false, -2560)]).is_empty());
        // Stream ended: the virtual display is gone, positions differ on purpose.
        assert!(moved_displays(&saved, &[at("LG", false, 0)]).is_empty());
    }

    #[test]
    fn session_round_trip() {
        let dir = std::env::temp_dir().join(format!("urakata-test-{}", std::process::id()));
        let path = dir.join("session.json");
        assert!(load_session(&path).unwrap().is_none());
        save_session(&path, &Session::now(vec![display("LG", false, None)])).unwrap();
        let s = load_session(&path).unwrap().unwrap();
        assert_eq!(s.displays[0].monitor_path, "LG");
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(load_session(&path), Err(LoadError::Damaged(_))));
        remove_session(&path).unwrap();
        remove_session(&path).unwrap();
        assert!(load_session(&path).unwrap().is_none());
        let _ = std::fs::remove_dir(&dir);
    }
}
