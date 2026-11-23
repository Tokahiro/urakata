//! urakata: display helper for Steam Remote Play on Windows.
//!
//! `urakata status [--json]`: a read-only report of the SudoVDA driver and
//! the display topology.
//!
//! `urakata exclusive` and `urakata restore`, by hand. They change the
//! display layout, never the driver or Steam's displays.
//!
//! `urakata watch` runs them by itself when a stream starts
//! and ends, and can start at logon.
//!
//! `urakata steam-entry` adds the settings window to the Steam library as a
//! non-Steam game.

use urakata::driver::{self, Action};
use urakata::shortcuts::EntryAction;
#[cfg(windows)]
use urakata::{display, status, steam_entry, watcher};

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "\
Usage: urakata status [--json]
       urakata exclusive [--dry-run]
       urakata restore [--dry-run]
       urakata watch [--install | --uninstall]
       urakata driver status [--json]
       urakata driver install | reinstall | repair [--package <dir>] [--log <file>]
       urakata driver uninstall [--keep-certificate] [--log <file>]
       urakata update [--check]
       urakata steam-entry [--add | --remove]
       urakata --version

Commands:
  status      Show the SudoVDA driver state, the active displays and setup
              checks (read-only).
  exclusive   During a stream: turn off all displays except the SudoVDA ones.
              Saves the layout first.
  restore     Turn the displays on again in the layout Windows stored for them,
              and set HDR back to the saved state.
  watch       Run `exclusive` when a stream starts. After it ends, delete the
              saved layout, or restore it if Windows did not. At start
              without a stream, repair a layout that a crash left behind.
              Logs to %LOCALAPPDATA%\\urakata\\watch.log.
  driver      Check the SudoVDA driver (status), install what is missing
              (install), install it again (reinstall), fix a broken setup
              (repair), or remove it (uninstall). All but status need admin
              and refuse while a stream runs.
  update      Look for a newer release on GitHub; download it, check its
              SHA-256 and run its installer silently (keeps the driver and
              the logon start). Refuses while a stream runs.
  steam-entry Show whether the Steam library has the entry \"urakata\" (a
              non-Steam game that opens the settings window), for each
              Steam account on this PC. --add also sets its icon and
              library images. Steam must not run for --add and --remove.

Options:
  --json      Print the status as JSON.
  --dry-run   Check the change with Windows, but change nothing.
  --install   Start the watcher (urakata-watch.exe, no window) at each logon
              of this user (no admin needed).
  --uninstall Do not start `watch` at logon any more.
  --package   Folder with the SudoVDA package (default: the `driver` folder
              next to urakata.exe). Its files must match the pinned hashes.
  --keep-certificate
              Leave the driver certificate in the machine stores.
  --log       Driver changes: also write the output and exit code to this
              file.
  --check     Only say whether a newer release exists.
  --add       Add the entry, or correct its program path. Keeps the list
              from before urakata's first change as
              shortcuts.vdf.urakata-backup.
  --remove    Remove the entry.
  -h, --help  Show this help.
  -V, --version
              Show the version of urakata.

Exit codes:
  0  Success.
  1  The output could not be made or written.
  2  Usage error.
  3  The status was printed, but some parts could not be read (see errors),
     or restore finished with warnings, or the driver still needs a repair
     after a driver command, or steam-entry failed for a Steam account.
  10 driver status: SudoVDA is not installed.
  11 driver status: SudoVDA needs a repair.";

/// Exit code when the report was printed but lists errors.
const EXIT_PARTIAL: u8 = 3;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Status { json: bool },
    Exclusive { dry_run: bool },
    Restore { dry_run: bool },
    Watch,
    WatchInstall,
    WatchUninstall,
    Driver(DriverCommand),
    Update { check_only: bool },
    SteamEntry(EntryAction),
}

#[derive(Debug, PartialEq, Eq)]
enum DriverCommand {
    Status {
        json: bool,
    },
    Change {
        action: Action,
        package: Option<std::path::PathBuf>,
        /// Also write the output to this file (for the installer, which
        /// runs the command elevated and cannot read its console).
        log: Option<std::path::PathBuf>,
    },
}

fn parse_driver<'a>(mut args: impl Iterator<Item = &'a str>) -> Result<Command, String> {
    let sub = args.next().ok_or("missing driver command")?;
    let mut rest: Vec<&str> = args.collect();
    let log = match rest.iter().position(|a| *a == "--log") {
        Some(_) if sub == "status" => return Err("unknown option '--log'".into()),
        Some(i) if i + 1 < rest.len() => {
            let file = rest.remove(i + 1);
            rest.remove(i);
            Some(std::path::PathBuf::from(file))
        }
        Some(_) => return Err("'--log' needs a file".into()),
        None => None,
    };
    if rest.iter().any(|a| matches!(*a, "-h" | "--help")) {
        return Ok(Command::Help);
    }
    let action = match sub {
        "-h" | "--help" => return Ok(Command::Help),
        "status" => {
            return match rest.as_slice() {
                [] => Ok(Command::Driver(DriverCommand::Status { json: false })),
                ["--json"] => Ok(Command::Driver(DriverCommand::Status { json: true })),
                [other, ..] => Err(format!("unknown option '{other}'")),
            };
        }
        "install" => Action::Install,
        "reinstall" => Action::Reinstall,
        "repair" => Action::Repair,
        "uninstall" => {
            let keep_certificate = match rest.as_slice() {
                [] => false,
                ["--keep-certificate"] => true,
                [other, ..] => return Err(format!("unknown option '{other}'")),
            };
            return Ok(Command::Driver(DriverCommand::Change {
                action: Action::Uninstall { keep_certificate },
                package: None,
                log,
            }));
        }
        other => return Err(format!("unknown driver command '{other}'")),
    };
    let package = match rest.as_slice() {
        [] => None,
        ["--package", dir] => Some(std::path::PathBuf::from(dir)),
        ["--package"] => return Err("'--package' needs a folder".into()),
        [other, ..] => return Err(format!("unknown option '{other}'")),
    };
    Ok(Command::Driver(DriverCommand::Change {
        action,
        package,
        log,
    }))
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut args = args.iter().map(String::as_str);
    let command = match args.next() {
        None => return Err("missing command".into()),
        Some("-h" | "--help" | "help") => return Ok(Command::Help),
        Some("-V" | "--version") => {
            return match args.next() {
                None => Ok(Command::Version),
                Some(other) => Err(format!("unknown option '{other}'")),
            };
        }
        Some("driver") => return parse_driver(args),
        Some("update") => {
            let rest: Vec<&str> = args.collect();
            return match rest.as_slice() {
                [] => Ok(Command::Update { check_only: false }),
                ["--check"] => Ok(Command::Update { check_only: true }),
                [a, ..] if matches!(*a, "-h" | "--help") => Ok(Command::Help),
                [other, ..] => Err(format!("unknown option '{other}'")),
            };
        }
        Some(c @ ("status" | "exclusive" | "restore" | "watch" | "steam-entry")) => c,
        Some(other) => return Err(format!("unknown command '{other}'")),
    };
    let allowed: &[&str] = match command {
        "status" => &["--json"],
        "watch" => &["--install", "--uninstall"],
        "steam-entry" => &["--add", "--remove"],
        _ => &["--dry-run"],
    };
    let mut flag = None;
    for arg in args {
        match arg {
            "-h" | "--help" => return Ok(Command::Help),
            a if allowed.contains(&a) && flag.is_none_or(|f| f == a) => flag = Some(a),
            a if allowed.contains(&a) => {
                return Err(format!("'{a}' conflicts with another option"));
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    let set = flag.is_some();
    Ok(match (command, flag) {
        ("status", _) => Command::Status { json: set },
        ("exclusive", _) => Command::Exclusive { dry_run: set },
        ("restore", _) => Command::Restore { dry_run: set },
        ("steam-entry", Some("--add")) => Command::SteamEntry(EntryAction::Add),
        ("steam-entry", Some(_)) => Command::SteamEntry(EntryAction::Remove),
        ("steam-entry", None) => Command::SteamEntry(EntryAction::Status),
        (_, Some("--install")) => Command::WatchInstall,
        (_, Some(_)) => Command::WatchUninstall,
        (_, None) => Command::Watch,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("urakata: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Command::Driver(cmd) = command {
        return ExitCode::from(driver_main(cmd));
    }
    let result = match command {
        Command::Driver(_) => unreachable!("handled above"),
        Command::Update { check_only } => update(check_only).map(|t| (t, false)),
        Command::Help => Ok((USAGE.to_string(), false)),
        Command::Version => Ok((format!("urakata {}", urakata::update::CURRENT), false)),
        Command::Status { json } => status_text(json),
        Command::Exclusive { dry_run } => change(dry_run, Change::Exclusive),
        Command::Restore { dry_run } => change(dry_run, Change::Restore),
        Command::Watch => watch().map(|()| (String::new(), false)),
        Command::WatchInstall => autostart(true).map(|t| (t, false)),
        Command::WatchUninstall => autostart(false).map(|t| (t, false)),
        Command::SteamEntry(which) => steam_entry(which),
    };
    let (text, has_errors) = match result {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("urakata: {msg}");
            return ExitCode::FAILURE;
        }
    };
    // A closed pipe (for example `| head`) is not a crash.
    let written = writeln!(std::io::stdout().lock(), "{text}").is_ok();
    ExitCode::from(exit_code(written, has_errors))
}

/// Exit code after the output was written (or not).
fn exit_code(written: bool, has_errors: bool) -> u8 {
    match (written, has_errors) {
        (false, _) => 1,
        (true, true) => EXIT_PARTIAL,
        (true, false) => 0,
    }
}

/// The status output and whether the report lists errors.
#[cfg(windows)]
fn status_text(json: bool) -> Result<(String, bool), String> {
    let report = status::collect();
    let text = if json {
        serde_json::to_string_pretty(&report).map_err(|e| format!("cannot write JSON: {e}"))?
    } else {
        report.to_text()
    };
    Ok((text, !report.errors.is_empty()))
}

#[cfg(not(windows))]
fn status_text(_json: bool) -> Result<(String, bool), String> {
    Err("status works on Windows only".into())
}

enum Change {
    Exclusive,
    Restore,
}

/// Runs a layout change. Returns its output and whether it has warnings.
#[cfg(windows)]
fn change(dry_run: bool, which: Change) -> Result<(String, bool), String> {
    let outcome = match which {
        Change::Exclusive => display::exclusive(dry_run, false)?,
        Change::Restore => display::restore(dry_run)?,
    };
    Ok((outcome.text, outcome.partial))
}

#[cfg(not(windows))]
fn change(_dry_run: bool, _which: Change) -> Result<(String, bool), String> {
    Err("this command works on Windows only".into())
}

#[cfg(windows)]
fn watch() -> Result<(), String> {
    watcher::watch()
}

#[cfg(not(windows))]
fn watch() -> Result<(), String> {
    Err("this command works on Windows only".into())
}

#[cfg(windows)]
fn autostart(on: bool) -> Result<String, String> {
    if on {
        watcher::install()
    } else {
        watcher::uninstall()
    }
}

#[cfg(not(windows))]
fn autostart(_on: bool) -> Result<String, String> {
    Err("this command works on Windows only".into())
}

#[cfg(windows)]
fn steam_entry(which: EntryAction) -> Result<(String, bool), String> {
    steam_entry::run(which)
}

#[cfg(not(windows))]
fn steam_entry(_which: EntryAction) -> Result<(String, bool), String> {
    Err("this command works on Windows only".into())
}

/// Runs a driver command and returns the exit code.
fn driver_main(cmd: DriverCommand) -> u8 {
    let log = match &cmd {
        DriverCommand::Change { log, .. } => log.clone(),
        DriverCommand::Status { .. } => None,
    };
    let (text, code) = match driver_run(cmd) {
        Ok((text, code)) => {
            let written = writeln!(std::io::stdout().lock(), "{text}").is_ok();
            (text, if written { code } else { 1 })
        }
        Err(msg) => {
            eprintln!("urakata: {msg}");
            (format!("urakata: {msg}"), 1)
        }
    };
    if let Some(path) = log
        && let Err(e) = std::fs::write(&path, format!("{text}\r\nexit code {code}\r\n"))
    {
        eprintln!("urakata: cannot write {}: {e}", path.display());
    }
    code
}

/// Exit code for a driver health.
#[cfg_attr(not(windows), allow(dead_code))]
fn health_code(health: driver::Health) -> u8 {
    match health {
        driver::Health::Ok => 0,
        driver::Health::Missing => 10,
        driver::Health::Broken => 11,
    }
}

#[cfg(windows)]
fn driver_run(cmd: DriverCommand) -> Result<(String, u8), String> {
    let state = driver::read_state()?;
    let (action, package) = match cmd {
        DriverCommand::Status { json } => {
            let code = health_code(state.health());
            let text = if json {
                let value = serde_json::json!({
                    "health": state.health(),
                    "bundled_driver_version": driver::PACKAGE_DRIVER_VERSION,
                    "state": state,
                });
                serde_json::to_string_pretty(&value)
                    .map_err(|e| format!("cannot write JSON: {e}"))?
            } else {
                driver::status_text(&state)
            };
            return Ok((text, code));
        }
        DriverCommand::Change {
            action, package, ..
        } => (action, package),
    };
    let steps = driver::plan(action, &state).map_err(|e| e.to_string())?;
    if steps.is_empty() {
        // Install leaves a present but broken node to `repair`.
        let code = match (action, state.health()) {
            (Action::Uninstall { .. }, _) | (_, driver::Health::Ok) => 0,
            _ => 3,
        };
        return Ok((
            format!("{}\n{}", driver::NOTHING_TO_DO, driver::status_text(&state)),
            code,
        ));
    }
    let needs_package = steps.iter().any(|s| {
        matches!(
            s,
            driver::Step::AddCertificate { .. } | driver::Step::InstallDriver { .. }
        )
    });
    let dir = match package {
        Some(d) => d,
        None => std::env::current_exe()
            .map_err(|e| format!("cannot find this program: {e}"))?
            .with_file_name("driver"),
    };
    // Holds the package files locked until the install is done.
    let package = if needs_package {
        Some(driver::verify_package(&dir)?)
    } else {
        None
    };
    let cert = package.as_ref().map_or(&[][..], |p| p.cert.as_slice());
    let (applied, failure) = match driver::apply(&steps, &dir, cert) {
        Ok(a) => (a, None),
        Err((a, e)) => (a, Some(e)),
    };
    let mut out = applied.log.join("\n");
    if let Some(e) = failure {
        return Err(format!("{out}\nfailed: {e}").trim_start().to_string());
    }
    // The device can need a moment to start after the install.
    let mut after = driver::read_state()?;
    for _ in 0..10 {
        if after.health() == driver::Health::Ok || matches!(action, Action::Uninstall { .. }) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        after = driver::read_state()?;
    }
    out.push('\n');
    out.push_str(&driver::status_text(&after));
    if applied.reboot {
        out.push('\n');
        out.push_str(driver::RESTART_NOTE);
    }
    let code = match action {
        Action::Uninstall { .. } if after.devices.is_empty() => 0,
        Action::Uninstall { .. } => 3,
        _ if after.health() == driver::Health::Ok => {
            if let Some(warning) = protocol_warning() {
                out.push('\n');
                out.push_str(&warning);
            }
            0
        }
        _ => 3,
    };
    Ok((out, code))
}

/// Steam refuses SudoVDA protocol major 1 (seen in tests), so warn on any major
/// other than 0. No stream runs here (the plan refuses one), so the query's
/// watchdog reset does no harm.
#[cfg(windows)]
fn protocol_warning() -> Option<String> {
    let device = sudovda::Device::open_read_only().ok()?;
    let v = device.protocol_version().ok()?;
    (v.major != 0).then(|| {
        format!(
            "Warning: the driver reports protocol {}.{}.{}; Steam accepts only major 0.",
            v.major, v.minor, v.incremental
        )
    })
}

#[cfg(windows)]
fn update(check_only: bool) -> Result<String, String> {
    use urakata::update;
    match update::check()? {
        None => Ok(format!("urakata {} is up to date.", update::CURRENT)),
        Some(r) if check_only => Ok(format!(
            "urakata {} is available (this is {}). Run `urakata update` to install it.",
            r.version,
            update::CURRENT
        )),
        Some(r) => update::install(&r, false),
    }
}

#[cfg(not(windows))]
fn update(_check_only: bool) -> Result<String, String> {
    Err("this command works on Windows only".into())
}

#[cfg(not(windows))]
fn driver_run(_cmd: DriverCommand) -> Result<(String, u8), String> {
    Err("this command works on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_status() {
        assert_eq!(
            parse(&args(&["status"])),
            Ok(Command::Status { json: false })
        );
        assert_eq!(
            parse(&args(&["status", "--json"])),
            Ok(Command::Status { json: true })
        );
        assert_eq!(parse(&args(&["status", "-h"])), Ok(Command::Help));
        assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
    }

    #[test]
    fn parse_version() {
        assert_eq!(parse(&args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse(&args(&["-V"])), Ok(Command::Version));
        assert!(parse(&args(&["--version", "--json"])).is_err());
    }

    #[test]
    fn parse_layout_commands() {
        assert_eq!(
            parse(&args(&["exclusive"])),
            Ok(Command::Exclusive { dry_run: false })
        );
        assert_eq!(
            parse(&args(&["exclusive", "--dry-run"])),
            Ok(Command::Exclusive { dry_run: true })
        );
        assert_eq!(
            parse(&args(&["restore", "--dry-run"])),
            Ok(Command::Restore { dry_run: true })
        );
        assert!(parse(&args(&["restore", "--json"])).is_err());
        assert!(parse(&args(&["status", "--dry-run"])).is_err());
    }

    #[test]
    fn parse_watch() {
        assert_eq!(parse(&args(&["watch"])), Ok(Command::Watch));
        assert_eq!(
            parse(&args(&["watch", "--install"])),
            Ok(Command::WatchInstall)
        );
        assert_eq!(
            parse(&args(&["watch", "--uninstall"])),
            Ok(Command::WatchUninstall)
        );
        assert!(parse(&args(&["watch", "--install", "--uninstall"])).is_err());
        assert!(parse(&args(&["watch", "--dry-run"])).is_err());
        assert!(parse(&args(&["restore", "--install"])).is_err());
    }

    #[test]
    fn parse_steam_entry() {
        assert_eq!(
            parse(&args(&["steam-entry"])),
            Ok(Command::SteamEntry(EntryAction::Status))
        );
        assert_eq!(
            parse(&args(&["steam-entry", "--add"])),
            Ok(Command::SteamEntry(EntryAction::Add))
        );
        assert_eq!(
            parse(&args(&["steam-entry", "--remove"])),
            Ok(Command::SteamEntry(EntryAction::Remove))
        );
        assert!(parse(&args(&["steam-entry", "--add", "--remove"])).is_err());
        assert!(parse(&args(&["steam-entry", "--install"])).is_err());
        assert!(parse(&args(&["watch", "--add"])).is_err());
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["start"])).is_err());
        assert!(parse(&args(&["status", "--yaml"])).is_err());
    }

    #[test]
    fn parse_driver_commands() {
        let change = |action, package: Option<&str>| {
            Ok(Command::Driver(DriverCommand::Change {
                action,
                package: package.map(Into::into),
                log: None,
            }))
        };
        assert_eq!(
            parse(&args(&["driver", "status"])),
            Ok(Command::Driver(DriverCommand::Status { json: false }))
        );
        assert_eq!(
            parse(&args(&["driver", "status", "--json"])),
            Ok(Command::Driver(DriverCommand::Status { json: true }))
        );
        assert_eq!(
            parse(&args(&["driver", "install"])),
            change(Action::Install, None)
        );
        assert_eq!(
            parse(&args(&["driver", "reinstall", "--package", r"C:\pkg"])),
            change(Action::Reinstall, Some(r"C:\pkg"))
        );
        assert_eq!(
            parse(&args(&["driver", "repair"])),
            change(Action::Repair, None)
        );
        assert_eq!(
            parse(&args(&["driver", "uninstall", "--keep-certificate"])),
            change(
                Action::Uninstall {
                    keep_certificate: true
                },
                None
            )
        );
        assert_eq!(parse(&args(&["driver", "repair", "-h"])), Ok(Command::Help));
        assert_eq!(
            parse(&args(&[
                "driver",
                "repair",
                "--log",
                "x.log",
                "--package",
                "p"
            ])),
            Ok(Command::Driver(DriverCommand::Change {
                action: Action::Repair,
                package: Some("p".into()),
                log: Some("x.log".into()),
            }))
        );
        assert!(parse(&args(&["driver", "repair", "--log"])).is_err());
        assert!(parse(&args(&["driver", "status", "--log", "x"])).is_err());
        assert!(parse(&args(&["driver"])).is_err());
        assert!(parse(&args(&["driver", "update"])).is_err());
        assert!(parse(&args(&["driver", "install", "--package"])).is_err());
        assert!(parse(&args(&["driver", "install", "--json"])).is_err());
        assert!(parse(&args(&["driver", "status", "--package", "x"])).is_err());
        assert!(parse(&args(&["driver", "uninstall", "--package", "x"])).is_err());
    }

    #[test]
    fn parse_update() {
        assert_eq!(
            parse(&args(&["update"])),
            Ok(Command::Update { check_only: false })
        );
        assert_eq!(
            parse(&args(&["update", "--check"])),
            Ok(Command::Update { check_only: true })
        );
        assert_eq!(parse(&args(&["update", "-h"])), Ok(Command::Help));
        assert!(parse(&args(&["update", "--json"])).is_err());
        assert!(parse(&args(&["update", "--check", "--check"])).is_err());
    }

    #[test]
    fn driver_health_codes() {
        assert_eq!(health_code(driver::Health::Ok), 0);
        assert_eq!(health_code(driver::Health::Missing), 10);
        assert_eq!(health_code(driver::Health::Broken), 11);
        assert!(USAGE.contains("  10 driver status: SudoVDA is not installed."));
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(true, false), 0);
        assert_eq!(exit_code(true, true), 3);
        assert_eq!(exit_code(false, false), 1);
        assert_eq!(exit_code(false, true), 1);
        assert!(USAGE.contains("  3  The status was printed"));
    }
}
