//! urakata library: the parts that the `urakata` command and the
//! `urakata-watch` background program share.

#[cfg(windows)]
mod ccd;
pub mod checks;
#[cfg(windows)]
pub mod display;
pub mod driver;
mod layout;
pub mod report;
pub mod settings;
pub mod shortcuts;
#[cfg(windows)]
pub mod status;
pub mod steam_art;
#[cfg(windows)]
pub mod steam_entry;
#[cfg(windows)]
pub mod tray;
pub mod update;
mod watch;
#[cfg(windows)]
pub mod watcher;

pub use layout::data_dir;
