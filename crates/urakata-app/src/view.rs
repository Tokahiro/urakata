//! What the window shows, built from the status report and the settings.
//! No Windows API here, so the tests run anywhere.

use urakata::checks::{Check, Level};
use urakata::driver::{self, Health};
use urakata::report::Device;
use urakata::settings::Settings;
use urakata::update::{self, Release};

/// One status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub title: String,
    pub level: &'static str,
    pub message: String,
}

/// Status lines for the checks. The watcher state has its own section in
/// the window, so its check is left out.
pub fn rows(checks: &[Check]) -> Vec<Row> {
    checks
        .iter()
        .filter(|c| c.name != "watcher_running")
        .map(|c| Row {
            title: title(c.name).to_string(),
            level: level(c.level),
            message: for_window(&c.message),
        })
        .collect()
}

fn title(name: &str) -> &str {
    match name {
        "sudovda_driver" => "SudoVDA driver",
        "steam_primary_display" => "Steam primary display",
        "steam_nvfbc" => "Steam NVFBC capture",
        "steam_debug_port" => "Steam debug port",
        "watcher_autostart" => "Start at logon",
        "saved_layout" => "Saved layout",
        other => other,
    }
}

fn level(level: Level) -> &'static str {
    match level {
        Level::Ok => "ok",
        Level::Warn => "warn",
        Level::Unknown => "unknown",
    }
}

/// The check messages name the commands for the console. In the window,
/// the same action is a control below the status.
fn for_window(message: &str) -> String {
    const SWAPS: [(&str, &str); 4] = [
        (
            "run `urakata driver repair` as admin",
            "use the driver card on the Displays page",
        ),
        (
            "run `urakata watch --install` again",
            "switch \"Start at logon\" off and on again",
        ),
        (
            "run `urakata watch --install`",
            "switch on \"Start at logon\" below",
        ),
        ("run `urakata restore`", "press Restore below"),
    ];
    let mut text = message.to_string();
    for (from, to) in SWAPS {
        text = text.replace(from, to);
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// The line above the checks: does a stream run now?
pub fn stream_text(devices: Option<&[Device]>) -> String {
    let Some(devices) = devices else {
        return "Stream state not read.".into();
    };
    let mut count = 0;
    for d in devices {
        match &d.monitors {
            Some(m) => count += m.len(),
            None => return "Stream state not read.".into(),
        }
    }
    match count {
        0 => "No stream runs.".into(),
        1 => "A stream runs (1 SudoVDA display).".into(),
        n => format!("A stream runs ({n} SudoVDA displays)."),
    }
}

/// Settings from the window's fields. A spin box value below 0 is 0.
pub fn settings_from(
    exclusive: bool,
    start_delay: i32,
    guard: i32,
    reapply: i32,
    tray_icon: bool,
) -> Settings {
    let n = |v: i32| u32::try_from(v).unwrap_or(0);
    Settings {
        exclusive,
        start_delay_s: n(start_delay),
        guard_s: n(guard),
        guard_reapply: n(reapply),
        tray_icon,
    }
    .clamped()
}

/// The driver section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverView {
    pub line: String,
    pub details: String,
    /// Text of the repair button.
    pub button: &'static str,
    pub can_repair: bool,
}

/// The driver section for a read of the driver state.
pub fn driver_view(state: Result<&driver::State, &str>) -> DriverView {
    let state = match state {
        Ok(s) => s,
        Err(e) => {
            return DriverView {
                line: "Driver state not read".into(),
                details: for_window(e),
                button: "Repair",
                can_repair: false,
            };
        }
    };
    let (line, button) = match state.health() {
        Health::Ok => ("The SudoVDA driver works.", "Repair"),
        Health::Missing => ("The SudoVDA driver is not installed.", "Install"),
        Health::Broken => ("The SudoVDA driver needs a repair.", "Repair"),
    };
    let needed = state.health() != Health::Ok;
    let can_repair = needed && !state.stream_active;
    // The first line of the console text is the health, shown above.
    let mut details: Vec<String> = driver::status_text(state)
        .lines()
        .skip(1)
        .map(|l| l.trim().to_string())
        .collect();
    if can_repair {
        details.push("Windows asks for admin rights.".into());
    } else if needed {
        details.push("End the stream first.".into());
    }
    DriverView {
        line: line.into(),
        details: details.join("\n"),
        button,
        can_repair,
    }
}

/// The notice for `urakata driver repair` with its exit code and the text
/// of its `--log` file.
pub fn repair_result(code: u32, log: Option<&str>) -> Result<String, String> {
    let log = log.unwrap_or("");
    match code {
        0 => {
            let mut text = if log.starts_with(driver::NOTHING_TO_DO) {
                "The driver needed no repair.".to_string()
            } else {
                "The driver was repaired.".to_string()
            };
            if log.contains(driver::RESTART_NOTE) {
                text.push(' ');
                text.push_str(driver::RESTART_NOTE);
            }
            Ok(text)
        }
        3 => Err("The repair ran, but the driver still needs a repair. \
                  See driver-repair.log in the log folder."
            .into()),
        _ => {
            // The log ends with the error line, then "exit code N".
            let reason = log
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty() && !l.starts_with("exit code"))
                .map(|l| l.strip_prefix("urakata: ").unwrap_or(l));
            Err(match reason {
                Some(r) => for_window(&format!("The repair failed: {r}")),
                None => format!("The repair failed (exit code {code})."),
            })
        }
    }
}

/// The notice for an update check.
pub fn update_text(release: Option<&Release>) -> String {
    match release {
        None => format!("urakata {} is up to date.", update::CURRENT),
        Some(r) => format!(
            "urakata {} is available (this is {}). Install it below.",
            r.version,
            update::CURRENT
        ),
    }
}

/// The notice after the installer started. The installer closes this
/// window; it opens it again unless the window runs in Big Picture.
pub fn installing_text(version: &str, reopens: bool) -> String {
    if reopens {
        format!(
            "Installing urakata {version}. This window closes and opens again when the update is done."
        )
    } else {
        format!(
            "Installing urakata {version}. This window closes; open it again when the update is done."
        )
    }
}

/// The notice when the update did not start.
pub fn install_error(error: &str) -> String {
    format!(
        "The update did not start: {}.",
        error.trim_end().trim_end_matches('.')
    )
}

/// The window's own size in logical pixels (`ui/app.slint`).
const WINDOW: (f32, f32) = (900.0, 600.0);

/// The scale for the full-screen window in Big Picture: as large as the
/// screen allows for the 900 x 600 layout, never below 1 nor above 4, in
/// steps of 0.05. `width` and `height` are the screen's physical pixels.
pub fn big_picture_scale(width: u32, height: u32) -> f32 {
    let fit = (width as f32 / WINDOW.0).min(height as f32 / WINDOW.1);
    ((fit * 20.0).floor() / 20.0).clamp(1.0, 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &'static str, level: Level, message: &str) -> Check {
        Check {
            name,
            level,
            message: message.into(),
        }
    }

    #[test]
    fn rows_skip_the_watcher_and_use_window_words() {
        let checks = [
            check("sudovda_driver", Level::Ok, "driver 1.10.9.289"),
            check("watcher_running", Level::Ok, "a watcher runs"),
            check(
                "watcher_autostart",
                Level::Warn,
                "the watcher does not start at logon: run `urakata watch --install`",
            ),
            check("new_check", Level::Unknown, ""),
        ];
        let rows = rows(&checks);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].title, "SudoVDA driver");
        assert_eq!(rows[0].level, "ok");
        assert_eq!(rows[0].message, "Driver 1.10.9.289");
        assert_eq!(
            rows[1].message,
            "The watcher does not start at logon: switch on \"Start at logon\" below"
        );
        assert_eq!(rows[2].title, "new_check");
        assert_eq!(rows[2].level, "unknown");
    }

    /// Builds the real messages of `checks::evaluate`, so that a changed
    /// message in checks.rs fails here instead of showing a console
    /// command in the window.
    #[test]
    fn real_check_messages_get_titles_and_no_commands() {
        use urakata::checks::{Autostart, Setup, evaluate};
        let idle = [device(Some(&[]))];
        let setups = [
            Setup {
                driver_health: Some(Health::Broken),
                steam_debug_flag: Some(true),
                autostart: Some(Autostart::default()),
                watcher_running: Some(false),
                session_file: Some(true),
                ..Setup::default()
            },
            Setup {
                autostart: Some(Autostart {
                    command: Some("x".into()),
                    program_exists: Some(false),
                    disabled: Some(false),
                }),
                ..Setup::default()
            },
        ];
        for setup in &setups {
            let checks = evaluate(Some(&idle), setup);
            for c in &checks {
                if c.name != "watcher_running" {
                    assert_ne!(title(c.name), c.name, "no title for {}", c.name);
                }
            }
            for row in rows(&checks) {
                assert!(!row.message.contains('`'), "{}", row.message);
            }
        }
    }

    fn device(monitors: Option<&[&str]>) -> Device {
        Device {
            monitors: monitors.map(|m| m.iter().map(|s| s.to_string()).collect()),
            ..Device::default()
        }
    }

    #[test]
    fn stream_line() {
        assert_eq!(stream_text(None), "Stream state not read.");
        assert_eq!(stream_text(Some(&[])), "No stream runs.");
        assert_eq!(stream_text(Some(&[device(Some(&[]))])), "No stream runs.");
        assert_eq!(
            stream_text(Some(&[device(Some(&["M1"]))])),
            "A stream runs (1 SudoVDA display)."
        );
        assert_eq!(
            stream_text(Some(&[device(Some(&["M1", "M2"]))])),
            "A stream runs (2 SudoVDA displays)."
        );
        assert_eq!(stream_text(Some(&[device(None)])), "Stream state not read.");
    }

    #[test]
    fn settings_from_fields() {
        assert_eq!(settings_from(true, 3, 20, 3, true), Settings::default());
        let s = settings_from(false, -5, 9999, 2, false);
        assert_eq!(s.start_delay_s, 0);
        assert_eq!(s.guard_s, urakata::settings::MAX_GUARD_S);
        assert!(!s.exclusive);
        assert!(!s.tray_icon);
    }

    fn healthy_device() -> driver::DeviceState {
        driver::DeviceState {
            instance_id: r"ROOT\DISPLAY\0000".into(),
            present: true,
            started: true,
            problem_code: None,
            driver_version: Some(driver::PACKAGE_DRIVER_VERSION.into()),
            driver_inf: Some("oem1.inf".into()),
        }
    }

    #[test]
    fn driver_section() {
        let ok = driver::State {
            devices: vec![healthy_device()],
            certificate: [true, true],
            ..driver::State::default()
        };
        let v = driver_view(Ok(&ok));
        assert_eq!(v.line, "The SudoVDA driver works.");
        assert!(!v.can_repair);
        assert!(v.details.starts_with(r"device ROOT\DISPLAY\0000: driver"));
        assert!(!v.details.contains('`'));

        let broken = driver::State {
            certificate: [true, false],
            ..ok.clone()
        };
        let v = driver_view(Ok(&broken));
        assert_eq!(v.line, "The SudoVDA driver needs a repair.");
        assert_eq!(v.button, "Repair");
        assert!(v.can_repair);
        assert!(v.details.contains("TrustedPublisher: missing"));
        assert!(v.details.ends_with("Windows asks for admin rights."));
        assert!(!v.details.contains('`'));

        let streaming = driver::State {
            stream_active: true,
            ..broken
        };
        let v = driver_view(Ok(&streaming));
        assert!(!v.can_repair);
        assert!(v.details.ends_with("End the stream first."));

        let v = driver_view(Ok(&driver::State::default()));
        assert_eq!(v.line, "The SudoVDA driver is not installed.");
        assert_eq!(v.button, "Install");
        assert!(v.can_repair);

        let v = driver_view(Err("access denied"));
        assert!(!v.can_repair);
        assert_eq!(v.details, "Access denied");
    }

    #[test]
    fn repair_results() {
        assert_eq!(
            repair_result(
                0,
                Some("Nothing to do.\nSudoVDA driver: ok\r\nexit code 0\r\n")
            ),
            Ok("The driver needed no repair.".into())
        );
        assert_eq!(
            repair_result(
                0,
                Some("installed\nSudoVDA driver: ok\nWindows asks for a restart to finish.")
            ),
            Ok("The driver was repaired. Windows asks for a restart to finish.".into())
        );
        assert!(
            repair_result(3, None)
                .unwrap_err()
                .contains("still needs a repair")
        );
        assert_eq!(
            repair_result(
                1,
                Some("urakata: a SudoVDA display is active (a stream runs); end the stream first\r\nexit code 1\r\n")
            ),
            Err("The repair failed: a SudoVDA display is active (a stream runs); end the stream first".into())
        );
        assert_eq!(
            repair_result(1, None),
            Err("The repair failed (exit code 1).".into())
        );
    }

    #[test]
    fn big_picture_scales() {
        assert_eq!(big_picture_scale(1920, 1080), 1.8);
        assert_eq!(big_picture_scale(3840, 2160), 3.6);
        // iPhone, wide: the height sets it.
        assert_eq!(big_picture_scale(2556, 1179), 1.95);
        assert_eq!(big_picture_scale(1280, 720), 1.2);
        assert_eq!(big_picture_scale(800, 600), 1.0);
        assert_eq!(big_picture_scale(7680, 4320), 4.0);
        assert_eq!(big_picture_scale(0, 0), 1.0);
    }

    #[test]
    fn update_texts() {
        assert!(update_text(None).ends_with("is up to date."));
        let r = Release {
            version: "9.0.0".into(),
            setup_name: String::new(),
            setup_url: String::new(),
            sha256_url: String::new(),
        };
        assert!(update_text(Some(&r)).starts_with("urakata 9.0.0 is available"));
        assert!(installing_text("9.0.0", true).contains("opens again"));
        assert!(!installing_text("9.0.0", false).contains("opens again"));
        assert_eq!(
            install_error("a SudoVDA display is active (a stream runs); update after the stream"),
            "The update did not start: a SudoVDA display is active (a stream runs); update after the stream."
        );
    }
}
