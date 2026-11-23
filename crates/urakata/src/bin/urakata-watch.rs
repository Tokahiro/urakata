//! `urakata-watch`: the watcher as a program without a console window, so
//! that the logon start opens no window. It does the same as
//! `urakata watch`, plus a tray icon (unless the settings hide it); its
//! output goes to `%LOCALAPPDATA%\urakata\watch.log`.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    match urakata::tray::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("urakata-watch: works on Windows only");
    ExitCode::FAILURE
}
