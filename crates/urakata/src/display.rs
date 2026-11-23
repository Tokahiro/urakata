//! `exclusive` and `restore` (Windows only).
//!
//! `exclusive` turns off every active display that is not a SudoVDA
//! display. It does not save the change to the Windows display database,
//! so the stored layout for the connected displays stays as it was.
//! `restore` applies that stored layout again (`SDC_USE_DATABASE_CURRENT`)
//! and then sets HDR back to the saved state.

use std::fmt::Write;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, SDC_ALLOW_CHANGES, SDC_APPLY,
    SDC_TOPOLOGY_EXTEND, SDC_USE_DATABASE_CURRENT, SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE,
};

use crate::ccd::{active_paths, set_advanced_color, set_config};
use crate::layout::{
    LoadError, Session, hdr_fixes, load_session, missing_displays, moved_displays, primary_offset,
    remap_modes, remove_session, save_session, session_path,
};
use crate::report::Display;
use crate::status::{displays_from, sudovda_monitor_ids};

/// Command output and whether a part of the work failed.
pub struct Outcome {
    pub text: String,
    pub partial: bool,
    /// The display layout was changed.
    pub changed: bool,
}

fn name(d: &Display) -> String {
    if d.monitor_name.is_empty() {
        d.gdi_name.clone()
    } else {
        format!("{} ({})", d.gdi_name, d.monitor_name)
    }
}

/// Makes the SudoVDA displays the only active displays. With
/// `keep_saved`, a valid saved layout is never replaced: the watcher's
/// guard runs when Steam has turned the physical displays on again, so
/// the current layout is Steam's stream layout, not the one to return to.
pub fn exclusive(dry_run: bool, keep_saved: bool) -> Result<Outcome, String> {
    let ids = sudovda_monitor_ids()?;
    let (paths, modes) = active_paths()?;
    let mut errors = Vec::new();
    let displays = displays_from(&paths, &modes, &ids, &mut errors);
    // A display whose monitor path could not be read might be a SudoVDA
    // display. Other read errors (for example HDR state) do not matter
    // for the choice.
    if displays.iter().any(|d| d.monitor_path.is_empty()) {
        return Err(format!("cannot read the displays: {}", errors.join("; ")));
    }
    let keep: Vec<usize> = (0..paths.len()).filter(|&i| displays[i].sudovda).collect();
    if keep.is_empty() {
        return Err("no SudoVDA display is active; start a stream first".into());
    }
    let mut text = String::new();
    if keep.len() == paths.len() {
        text.push_str("Only SudoVDA displays are active. Nothing to do.");
        return Ok(Outcome {
            changed: false,
            text,
            partial: false,
        });
    }

    // SAFETY (both unions): without QDC_VIRTUAL_MODE_AWARE the union
    // holds a plain mode index.
    let refs: Vec<(u32, u32)> = keep
        .iter()
        .map(|&i| unsafe {
            (
                paths[i].sourceInfo.Anonymous.modeInfoIdx,
                paths[i].targetInfo.Anonymous.modeInfoIdx,
            )
        })
        .collect();
    let remap = remap_modes(&refs, modes.len())?;
    let mut new_modes: Vec<_> = remap.modes.iter().map(|&i| modes[i]).collect();
    let new_paths: Vec<_> = keep
        .iter()
        .zip(&remap.refs)
        .map(|(&i, &(source, target))| {
            let mut p = paths[i];
            p.sourceInfo.Anonymous.modeInfoIdx = source;
            p.targetInfo.Anonymous.modeInfoIdx = target;
            p
        })
        .collect();

    // One of the kept sources must be at (0, 0), the primary position.
    let is_source = |m: &DISPLAYCONFIG_MODE_INFO| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
    // SAFETY: infoType says the union holds a source mode.
    let positions: Vec<(i32, i32)> = new_modes
        .iter()
        .filter(|m| is_source(m))
        .map(|m| unsafe {
            let p = m.Anonymous.sourceMode.position;
            (p.x, p.y)
        })
        .collect();
    let (dx, dy) = primary_offset(&positions);
    for m in new_modes.iter_mut().filter(|m| is_source(m)) {
        // SAFETY: infoType says the union holds a source mode.
        unsafe {
            m.Anonymous.sourceMode.position.x += dx;
            m.Anonymous.sourceMode.position.y += dy;
        }
    }

    let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
    set_config(&new_paths, &new_modes, flags | SDC_VALIDATE)?;

    let mut line = |s: String| {
        let _ = writeln!(text, "{s}");
    };
    for e in &errors {
        line(format!("Warning: {e}"));
    }
    for (i, d) in displays.iter().enumerate() {
        let verb = if keep.contains(&i) {
            "keep"
        } else {
            "turn off"
        };
        line(format!("{verb:8}  {}", name(d)));
    }
    if dry_run {
        line("Dry run: Windows accepts the layout. Nothing changed.".into());
        return Ok(Outcome {
            changed: false,
            text: text.trim_end().into(),
            partial: false,
        });
    }

    let file = session_path()?;
    // An earlier file is still the true "before" layout while one of its
    // physical displays is off. When all of them are active again, the
    // file is stale (for example the stream ended without `restore`).
    let earlier = match load_session(&file) {
        Ok(Some(old)) if keep_saved || !missing_displays(&old.displays, &displays).is_empty() => {
            Some(old)
        }
        Ok(_) => None,
        // A file that cannot be read now may still hold the true layout:
        // do not replace it. The watcher tries again.
        Err(LoadError::Read(e)) => return Err(e),
        Err(e) => {
            line(format!("Warning: {e}. Replacing it."));
            None
        }
    };
    let created = match earlier {
        Some(old) => {
            line(format!(
                "Keeping the layout saved {} in {}.",
                old.age(),
                file.display()
            ));
            false
        }
        None => {
            save_session(&file, &Session::now(displays))?;
            line(format!("Saved the layout to {}.", file.display()));
            true
        }
    };
    if let Err(e) = set_config(&new_paths, &new_modes, flags | SDC_APPLY) {
        // Nothing changed, so no session is active.
        if created {
            let _ = remove_session(&file);
        }
        return Err(e);
    }
    line("Done. Run `urakata restore` to turn the other displays on again.".into());
    Ok(Outcome {
        changed: true,
        text: text.trim_end().into(),
        partial: false,
    })
}

/// For the watcher, when no SudoVDA display is present. When a stream
/// ends during `exclusive`, Windows turns the physical displays on by
/// itself. Then only the saved file is left: delete it. When a saved
/// display is still off or its HDR state differs, run `restore`.
pub fn after_stream() -> Result<Outcome, String> {
    let file = session_path()?;
    let session = match load_session(&file) {
        Ok(None) => {
            return Ok(Outcome {
                changed: false,
                text: "No saved layout. Nothing to do.".into(),
                partial: false,
            });
        }
        Ok(Some(s)) => s,
        // A read error can pass; the watcher tries again. A damaged file
        // stays damaged: `restore` ignores and replaces it.
        Err(LoadError::Read(e)) => return Err(e),
        Err(LoadError::Damaged(_)) => return restore(false),
    };
    let (paths, modes) = active_paths()?;
    let mut errors = Vec::new();
    let current = displays_from(&paths, &modes, &[], &mut errors);
    if errors.is_empty()
        && missing_displays(&session.displays, &current).is_empty()
        && hdr_fixes(&session.displays, &current).is_empty()
    {
        remove_session(&file)?;
        return Ok(Outcome {
            changed: false,
            text: format!(
                "The saved displays are active again. Deleted {}.",
                file.display()
            ),
            partial: false,
        });
    }
    restore(false)
}

/// Applies the stored Windows layout for the connected displays and sets
/// HDR back to the saved state.
pub fn restore(dry_run: bool) -> Result<Outcome, String> {
    let file = session_path()?;
    let mut text = String::new();
    let mut partial = false;
    let mut line = |s: String| {
        let _ = writeln!(text, "{s}");
    };
    // A damaged or unknown file must not keep the displays off.
    let session = load_session(&file).unwrap_or_else(|e| {
        line(format!("Warning: {e}. Ignoring it."));
        None
    });
    match &session {
        Some(s) => line(format!("Layout saved {} ({}).", s.age(), file.display())),
        None => line("No saved layout. Applying the stored Windows layout anyway.".into()),
    }

    if dry_run {
        set_config(&[], &[], SDC_USE_DATABASE_CURRENT | SDC_VALIDATE)?;
        line("Dry run: Windows has a valid stored layout. Nothing changed.".into());
        return Ok(Outcome {
            changed: false,
            text: text.trim_end().into(),
            partial,
        });
    }
    match set_config(&[], &[], SDC_USE_DATABASE_CURRENT | SDC_APPLY) {
        Ok(()) => line("Applied the stored Windows layout for the connected displays.".into()),
        Err(e) => {
            line(format!(
                "{e}. Extending the desktop to all displays instead."
            ));
            set_config(&[], &[], SDC_TOPOLOGY_EXTEND | SDC_APPLY)?;
            partial = true;
        }
    }

    if let Some(session) = &session {
        let (paths, modes) = active_paths()?;
        let mut errors = Vec::new();
        // Only the saved list needs the SudoVDA flag.
        let current = displays_from(&paths, &modes, &[], &mut errors);
        for e in errors {
            line(format!("Warning: {e}"));
            partial = true;
        }
        for d in missing_displays(&session.displays, &current) {
            line(format!("Warning: {} is not active.", name(d)));
            partial = true;
        }
        for d in moved_displays(&session.displays, &current) {
            line(format!(
                "Warning: {} is at ({}, {}), not at the saved ({}, {}).",
                name(d.1),
                d.1.x,
                d.1.y,
                d.0.x,
                d.0.y
            ));
            partial = true;
        }
        for (i, enable) in hdr_fixes(&session.displays, &current) {
            let t = &paths[i].targetInfo;
            let state = if enable { "on" } else { "off" };
            match set_advanced_color(t.adapterId, t.id, enable) {
                Ok(()) => line(format!("HDR {state}: {}", name(&current[i]))),
                Err(e) => {
                    line(format!(
                        "Warning: HDR {state} for {}: {e}",
                        name(&current[i])
                    ));
                    partial = true;
                }
            }
        }
    }
    // Keep the file after a partial restore, so that a later `restore`
    // can finish the work.
    if partial {
        line(format!(
            "Kept {} for another `urakata restore`.",
            file.display()
        ));
    } else if let Err(e) = remove_session(&file) {
        line(format!("Warning: {e}"));
        partial = true;
    }
    line("Done.".into());
    Ok(Outcome {
        changed: true,
        text: text.trim_end().into(),
        partial,
    })
}
