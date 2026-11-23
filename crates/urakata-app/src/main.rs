//! `urakata-app`: the settings window. It shows the setup checks of
//! `urakata status`, starts and stops the watcher, switches its logon
//! start, and edits the settings that the watcher reads.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod pad;
#[cfg(any(windows, test))]
mod view;
#[cfg(windows)]
mod window;

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    window::run()
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("urakata-app: works on Windows only");
    ExitCode::FAILURE
}
