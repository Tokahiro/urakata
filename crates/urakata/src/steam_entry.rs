//! `urakata steam-entry`: the non-Steam game entry "urakata" in the Steam
//! library, which starts the settings window (`urakata-app.exe`) from Big
//! Picture and during a stream.
//!
//! The file rules live in `shortcuts.rs`. This module finds the files and
//! writes them. Steam keeps the list in memory and writes it on exit, so a
//! change while Steam runs is lost (Valve Developer Community, "Steam
//! Library Shortcuts": "Steam must be shut downed or it wont add the
//! shortcut"). `add` and `remove` therefore refuse while `steam.exe` runs.

use std::path::{Path, PathBuf};

use crate::shortcuts::{self, APP_EXE, Added, EntryAction as Action, Shortcuts};
use crate::steam_art;

/// Runs the command. Returns its output and whether an account failed.
pub fn run(action: Action) -> Result<(String, bool), String> {
    let steam =
        crate::status::steam_path()?.ok_or("Steam is not installed for this Windows user")?;
    let files = shortcut_files(&steam.join("userdata"))?;
    if files.is_empty() {
        return Err("no Steam account found on this PC (sign in to Steam once first)".into());
    }
    let exe = match action {
        Action::Add => Some(app_exe()?),
        _ => None,
    };
    if action != Action::Status && steam_running()? {
        return Err(
            "Steam is running. Exit Steam first (Steam > Exit); Steam writes its own list on exit"
                .into(),
        );
    }
    let total = files.len();
    let mut lines = Vec::new();
    let mut failed = false;
    let mut changed = false;
    for (n, file) in files.iter().enumerate() {
        // Account IDs stay out of the output, as in `urakata status`.
        let who = format!("Steam account {} of {total}", n + 1);
        let line = match one(action, file, exe.as_deref()) {
            Ok((text, done)) => {
                changed |= done;
                format!("{who}: {text}")
            }
            Err(e) => {
                failed = true;
                // A file may have changed before the error.
                changed |= action != Action::Status;
                format!("{who}: error: {e}")
            }
        };
        lines.push(line);
    }
    if changed {
        lines.push("Start Steam again to see the change.".into());
    }
    Ok((lines.join("\n"), failed))
}

/// The text for one account, and whether a file changed.
fn one(action: Action, file: &Path, exe: Option<&str>) -> Result<(String, bool), String> {
    let mut list = read(file)?;
    // Steam keeps the library images next to shortcuts.vdf.
    let grid = file.with_file_name("grid");
    match action {
        Action::Status => Ok(match list.find() {
            Some(_) => {
                let have: usize = list
                    .app_ids()
                    .iter()
                    .map(|&id| steam_art::present(&grid, id))
                    .max()
                    .unwrap_or(0);
                format!(
                    "entry present, artwork {have} of {} files",
                    steam_art::COUNT
                )
            }
            None => "no entry".into(),
        })
        .map(|text| (text, false)),
        Action::Add => {
            let exe = exe.expect("add has a program path");
            let added = list.add(exe);
            // The files first, so the icon field never points to a missing
            // file.
            let mut art = steam_art::Installed::default();
            for id in list.app_ids() {
                let one = steam_art::install(&grid, id)?;
                art.written += one.written;
                art.kept += one.kept;
            }
            let icon = list.set_icons(|id| {
                steam_art::icon_path(&grid, id)
                    .to_string_lossy()
                    .into_owned()
            });
            if added != Added::Unchanged || icon {
                write(file, &list)?;
            }
            let mut parts = vec![match added {
                Added::New => "entry added".into(),
                Added::Updated(c) => c.text(),
                Added::Unchanged => "entry present".into(),
            }];
            if icon && added != Added::New {
                parts.push("icon set".into());
            }
            if art.written > 0 {
                parts.push(format!("artwork added ({} files)", art.written));
            }
            if art.kept > 0 {
                parts.push(format!(
                    "your own artwork kept ({} files; to get urakata's back, \
                     delete them in Steam's grid folder and run --add again)",
                    art.kept
                ));
            }
            let done = added != Added::Unchanged || icon || art.written > 0;
            if !done {
                parts.push("nothing to do".into());
            }
            Ok((parts.join(", "), done))
        }
        Action::Remove => {
            // The list first: if it cannot be written, the entry and its
            // icon file both stay. The appids are read before.
            let ids = list.app_ids();
            let n = list.remove();
            if n > 0 {
                write(file, &list)?;
            }
            let mut art = 0;
            for id in ids {
                art += steam_art::remove(&grid, id)?;
            }
            let mut text = match n {
                0 => "no entry".to_string(),
                1 => "entry removed".to_string(),
                n => format!("{n} entries removed"),
            };
            if art > 0 {
                text.push_str(&format!(", artwork deleted ({art} files)"));
            }
            if n == 0 && art == 0 {
                text.push_str(", nothing to do");
            }
            Ok((text, n > 0 || art > 0))
        }
    }
}

/// `userdata\<id>\config\shortcuts.vdf` for each account folder with a
/// `config` folder. The file itself may not exist yet. Folder `0` is not an
/// account.
fn shortcut_files(userdata: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(userdata) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read Steam's userdata folder: {e}")),
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name != "0" && !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
        })
        .map(|e| e.path().join("config"))
        .filter(|config| config.is_dir())
        .map(|config| config.join("shortcuts.vdf"))
        .collect();
    files.sort();
    Ok(files)
}

fn read(file: &Path) -> Result<Shortcuts, String> {
    match std::fs::read(file) {
        Ok(bytes) => shortcuts::parse(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Shortcuts::default()),
        Err(e) => Err(format!("cannot read shortcuts.vdf: {e}")),
    }
}

/// Keeps a copy of the file as it was before urakata's first change (a later
/// change does not overwrite it), then replaces the file in one step. The
/// new data is on disk before the rename, so a crash or power loss leaves
/// the old or the new file, never half of one.
fn write(file: &Path, list: &Shortcuts) -> Result<(), String> {
    use std::io::Write;
    let backup = file.with_extension("vdf.urakata-backup");
    let temp = file.with_extension("vdf.urakata-new");
    if file.exists() && !backup.exists() {
        std::fs::copy(file, &backup).map_err(|e| format!("cannot copy shortcuts.vdf: {e}"))?;
    }
    let data = shortcuts::serialize(list);
    std::fs::File::create(&temp)
        .and_then(|mut f| {
            f.write_all(&data)?;
            f.sync_all()
        })
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("cannot write the new shortcuts.vdf: {e}")
        })?;
    std::fs::rename(&temp, file).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot replace shortcuts.vdf: {e}")
    })
}

/// `urakata-app.exe` next to this program, as the installer puts it.
fn app_exe() -> Result<String, String> {
    let me = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let exe = me.with_file_name(APP_EXE);
    if !exe.is_file() {
        return Err(format!(
            "{APP_EXE} not found next to urakata.exe; install the settings window first"
        ));
    }
    exe.into_os_string()
        .into_string()
        .map_err(|_| format!("the path of {APP_EXE} is not valid Unicode"))
}

/// Whether a `steam.exe` process runs (any Windows user: they share the
/// Steam folder).
fn steam_running() -> Result<bool, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: plain snapshot call; the handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| format!("cannot list processes: {e}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    // SAFETY: `entry` is a valid, sized PROCESSENTRY32W; `snap` is open.
    let first = unsafe { Process32FirstW(snap, &mut entry) };
    // A failed list must not count as "Steam is closed". An empty list
    // ends with ERROR_NO_MORE_FILES.
    if let Err(e) = &first
        && e.code() != ERROR_NO_MORE_FILES.to_hresult()
    {
        // SAFETY: handle from the call above.
        let _ = unsafe { CloseHandle(snap) };
        return Err(format!("cannot list processes: {e}"));
    }
    let mut more = first.is_ok();
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("steam.exe") {
            found = true;
            break;
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: handle from the call above.
    let _ = unsafe { CloseHandle(snap) };
    Ok(found)
}
