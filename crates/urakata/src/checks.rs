//! Setup checks: settings that the host tests showed to matter, judged
//! from facts that `status` reads. The rules build on every platform, so
//! the tests run anywhere; `status.rs` reads the facts on Windows.

#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;

use crate::driver::Health;
use crate::report::Device;

/// Driver build that the host tests ran with.
pub const TESTED_DRIVER: &str = "1.10.9.289";

/// Facts about the setup around the driver. A value that could not be read
/// is `null` and its reason is in the report's `errors`.
#[derive(Debug, Default, Serialize)]
pub struct Setup {
    /// The driver health of `urakata driver status`: device nodes, also
    /// ones that are not present, and the certificate stores.
    pub driver_health: Option<Health>,
    /// Steam's Remote Play host options.
    pub steam: Option<SteamSettings>,
    /// The watcher's logon start.
    pub autostart: Option<Autostart>,
    /// A watcher runs in this user session (its single-instance lock
    /// exists).
    pub watcher_running: Option<bool>,
    /// `%LOCALAPPDATA%\urakata\session.json` exists.
    pub session_file: Option<bool>,
    /// Steam's folder holds [`CEF_DEBUG_FLAG`]. `false` also when Steam is
    /// not installed.
    pub steam_debug_flag: Option<bool>,
}

/// File in Steam's folder that opens Steam's CEF remote debugging port
/// (127.0.0.1:8080) at Steam's next start.
pub const CEF_DEBUG_FLAG: &str = ".cef-enable-remote-debugging";

/// `SteamSettings::account` when no Steam account was found.
pub const NO_ACCOUNT: &str = "none";

/// Values of the `streaming_v2` block in the Steam account's
/// `localconfig.vdf`. Steam writes the file when an option changes.
#[derive(Debug, Default, Serialize)]
pub struct SteamSettings {
    /// How the account was chosen: `active` (Steam's running account),
    /// `only` (one account on this PC) or `newest` (the newest file of
    /// several). `none` when Steam or a signed-in account was not found;
    /// the options are then `null`. The account ID is left out on purpose.
    pub account: String,
    /// `CustomDisplayDevice`: "Primary display while streaming". `SUDOVDA`
    /// selects the virtual display.
    pub custom_display_device: Option<String>,
    /// `EnableCaptureNVFBC`: "Use NVFBC capture with NVIDIA GPU".
    pub enable_capture_nvfbc: Option<String>,
}

/// The `urakata` value in the per-user `Run` key.
#[derive(Debug, Default, Serialize)]
pub struct Autostart {
    /// The command, with the user profile path as `%USERPROFILE%`.
    pub command: Option<String>,
    /// The program in the command exists.
    pub program_exists: Option<bool>,
    /// The user switched it off in Settings > Apps > Startup.
    pub disabled: Option<bool>,
}

/// Result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// As recommended.
    Ok,
    /// Works, but not as tested or recommended.
    Warn,
    /// Not checked: a fact is missing.
    Unknown,
}

/// One setup check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Short stable name, for example `steam_nvfbc`.
    pub name: &'static str,
    /// The result.
    pub level: Level,
    /// What was found and, for a warning, what to do.
    pub message: String,
}

fn check(name: &'static str, level: Level, message: impl Into<String>) -> Check {
    Check {
        name,
        level,
        message: message.into(),
    }
}

/// Runs all checks. `devices` is the SudoVDA adapter list of the report.
pub fn evaluate(devices: Option<&[Device]>, setup: &Setup) -> Vec<Check> {
    let mut checks = vec![driver(devices, setup.driver_health)];
    checks.extend(steam(setup.steam.as_ref()));
    checks.push(steam_debug_port(setup.steam_debug_flag));
    checks.push(autostart(setup.autostart.as_ref()));
    checks.push(watcher(setup.watcher_running));
    checks.push(session(devices, setup.session_file));
    checks
}

fn driver(devices: Option<&[Device]>, health: Option<Health>) -> Check {
    const NAME: &str = "sudovda_driver";
    let list = match devices {
        None => return check(NAME, Level::Unknown, "adapter list not read"),
        Some([]) => {
            return check(
                NAME,
                Level::Warn,
                "SudoVDA is not installed: Steam cannot add a virtual display",
            );
        }
        Some(list) => list,
    };
    if list.len() > 1 {
        return check(
            NAME,
            Level::Warn,
            format!("{} SudoVDA adapters; one is expected", list.len()),
        );
    }
    let d = &list[0];
    if let Some(p) = d.problem_code {
        return check(
            NAME,
            Level::Warn,
            format!("adapter has problem code {p}; see Device Manager"),
        );
    }
    if d.started == Some(false) {
        return check(NAME, Level::Warn, "adapter is not started");
    }
    // The adapter alone can look fine while `urakata driver status` finds a
    // missing certificate or a left-over device node.
    if matches!(health, Some(Health::Missing | Health::Broken)) {
        return check(
            NAME,
            Level::Warn,
            "driver needs repair: run `urakata driver repair` as admin",
        );
    }
    let version = match d.driver_version.as_deref() {
        None => check(NAME, Level::Unknown, "driver version not read"),
        Some(TESTED_DRIVER) => check(NAME, Level::Ok, format!("driver {TESTED_DRIVER}")),
        Some(v) => check(
            NAME,
            Level::Warn,
            format!("driver {v}; urakata was tested with {TESTED_DRIVER}"),
        ),
    };
    // Without the health, an untested version still warns, but ok is not
    // known.
    if health.is_none() && version.level == Level::Ok {
        return check(NAME, Level::Unknown, "driver health not read");
    }
    version
}

fn steam(steam: Option<&SteamSettings>) -> [Check; 2] {
    const PRIMARY: &str = "steam_primary_display";
    const NVFBC: &str = "steam_nvfbc";
    let Some(s) = steam else {
        return [
            check(PRIMARY, Level::Unknown, "Steam settings not read"),
            check(NVFBC, Level::Unknown, "Steam settings not read"),
        ];
    };
    if s.account == NO_ACCOUNT {
        let why = "Steam or a Steam account was not found for this Windows user";
        return [
            check(PRIMARY, Level::Warn, why),
            check(NVFBC, Level::Unknown, why),
        ];
    }
    let primary = match s.custom_display_device.as_deref() {
        Some(v) if v.eq_ignore_ascii_case("SUDOVDA") => {
            check(PRIMARY, Level::Ok, "Steam streams on the SudoVDA display")
        }
        v => check(
            PRIMARY,
            Level::Warn,
            format!(
                "Steam's primary display while streaming is {}, not SudoVDA: set it in \
                 Steam > Settings > Remote Play > Advanced Host Options",
                match v {
                    None | Some("") => "the default".to_string(),
                    Some(v) => format!("'{v}'"),
                }
            ),
        ),
    };
    // In a test, NVFBC capture on the SudoVDA display broke the picture
    // (missing windows, then green); with 0 it worked. Without the key
    // Steam uses its own default, which was not tested.
    let nvfbc = match s.enable_capture_nvfbc.as_deref() {
        Some("0") => check(NVFBC, Level::Ok, "NVFBC capture is off"),
        Some("1") => check(
            NVFBC,
            Level::Warn,
            "NVFBC capture is on; on the SudoVDA display it can break the picture: \
             turn off \"Use NVFBC capture on NVIDIA GPU\" in Steam's Advanced Host Options",
        ),
        None => check(
            NVFBC,
            Level::Unknown,
            "NVFBC option not set; Steam's default is not known: set it off in \
             Steam's Advanced Host Options",
        ),
        Some(v) => check(NVFBC, Level::Unknown, format!("unknown NVFBC value '{v}'")),
    };
    [primary, nvfbc]
}

// The port accepts a WebSocket with a foreign `Origin`, so a web page
// might drive the Steam client too. urakata does not create the file yet,
// so it only warns and never deletes it.
fn steam_debug_port(flag: Option<bool>) -> Check {
    const NAME: &str = "steam_debug_port";
    match flag {
        None => check(NAME, Level::Unknown, "Steam folder not read"),
        Some(false) => check(
            NAME,
            Level::Ok,
            format!("no {CEF_DEBUG_FLAG} file in the Steam folder"),
        ),
        Some(true) => check(
            NAME,
            Level::Warn,
            format!(
                "Steam's remote debugging port is on (file {CEF_DEBUG_FLAG} in the Steam \
                 folder): other programs, and maybe web pages, can control the Steam client. \
                 If you do not need it, delete the file and restart Steam"
            ),
        ),
    }
}

fn autostart(a: Option<&Autostart>) -> Check {
    const NAME: &str = "watcher_autostart";
    let Some(a) = a else {
        return check(NAME, Level::Unknown, "logon start not read");
    };
    let Some(command) = &a.command else {
        return check(
            NAME,
            Level::Warn,
            "the watcher does not start at logon: run `urakata watch --install`",
        );
    };
    if a.program_exists == Some(false) {
        return check(
            NAME,
            Level::Warn,
            format!(
                "logon start points to a missing program ({command}): run `urakata watch --install` again"
            ),
        );
    }
    if a.disabled == Some(true) {
        return check(
            NAME,
            Level::Warn,
            "logon start is off in Settings > Apps > Startup",
        );
    }
    check(NAME, Level::Ok, format!("starts at logon: {command}"))
}

fn watcher(running: Option<bool>) -> Check {
    const NAME: &str = "watcher_running";
    match running {
        None => check(NAME, Level::Unknown, "not checked"),
        Some(true) => check(NAME, Level::Ok, "a watcher runs in this session"),
        Some(false) => check(
            NAME,
            Level::Warn,
            "no watcher runs: start `urakata-watch`, or log off and on",
        ),
    }
}

fn session(devices: Option<&[Device]>, file: Option<bool>) -> Check {
    const NAME: &str = "saved_layout";
    let Some(file) = file else {
        return check(NAME, Level::Unknown, "not checked");
    };
    if !file {
        return check(NAME, Level::Ok, "no saved layout");
    }
    // A stream runs while a SudoVDA monitor exists.
    let streaming = devices.map(|list| {
        list.iter()
            .any(|d| d.monitors.as_ref().is_some_and(|m| !m.is_empty()))
    });
    match streaming {
        Some(true) => check(NAME, Level::Ok, "saved layout of the running stream"),
        Some(false) => check(
            NAME,
            Level::Warn,
            "a saved layout is left from an earlier stream: the watcher removes it at \
             its next start, or run `urakata restore`",
        ),
        None => check(
            NAME,
            Level::Unknown,
            "saved layout exists; stream state not known",
        ),
    }
}

/// Replaces the user profile folder in `path` with `%USERPROFILE%`, so
/// that reports do not show the account name. Windows paths ignore case.
pub fn hide_profile(path: &str, profile: Option<&str>) -> String {
    let profile = profile.unwrap_or("").trim_end_matches('\\').to_lowercase();
    if profile.is_empty() {
        return path.to_string();
    }
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    'outer: while !rest.is_empty() {
        for (i, _) in rest.char_indices() {
            let tail = &rest[i..];
            let Some(len) = prefix_len_ignore_case(tail, &profile) else {
                continue;
            };
            // Only a whole folder name, not `C:\Users\ab` inside `C:\Users\abc`.
            if tail[len..]
                .chars()
                .next()
                .is_none_or(|c| matches!(c, '\\' | '"'))
            {
                out.push_str(&rest[..i]);
                out.push_str("%USERPROFILE%");
                rest = &tail[len..];
                continue 'outer;
            }
        }
        break;
    }
    out.push_str(rest);
    out
}

/// Byte length of the start of `text` that equals `lower` (already
/// lowercase) when lowercased char by char.
fn prefix_len_ignore_case(text: &str, lower: &str) -> Option<usize> {
    let mut want = lower.chars().peekable();
    for (i, c) in text.char_indices() {
        if want.peek().is_none() {
            return Some(i);
        }
        for l in c.to_lowercase() {
            if want.next() != Some(l) {
                return None;
            }
        }
    }
    want.peek().is_none().then_some(text.len())
}

/// The program path in a `Run` command: the quoted part, or the text up to
/// the first space.
pub fn command_program(command: &str) -> &str {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest);
    }
    command.split(' ').next().unwrap_or(command)
}

/// Key-value pairs of the first block named `name` (any depth, name
/// compared without case) in Valve's KeyValues text format. Nested blocks
/// inside it are skipped. `None` when there is no such block.
pub fn vdf_block(text: &str, name: &str) -> Option<Vec<(String, String)>> {
    let tokens = vdf_tokens(text);
    let mut i = 0;
    while i + 1 < tokens.len() {
        if let (Token::Str(key), Token::Open) = (&tokens[i], &tokens[i + 1]) {
            if key.eq_ignore_ascii_case(name) {
                return Some(block_pairs(&tokens[i + 2..]));
            }
        }
        i += 1;
    }
    None
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn block_pairs(tokens: &[Token]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Open => depth += 1,
            Token::Close if depth == 0 => break,
            Token::Close => depth -= 1,
            Token::Str(key) => {
                if let Some(Token::Str(value)) = tokens.get(i + 1) {
                    if depth == 0 {
                        pairs.push((key.clone(), value.clone()));
                    }
                    i += 1;
                }
            }
        }
        i += 1;
    }
    pairs
}

fn vdf_tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(n) = chars.next() {
                                s.push(match n {
                                    'n' => '\n',
                                    't' => '\t',
                                    other => other,
                                });
                            }
                        }
                        other => s.push(other),
                    }
                }
                tokens.push(Token::Str(s));
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            // Unquoted token (rare in Steam's files).
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || matches!(n, '{' | '}' | '"') {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                tokens.push(Token::Str(s));
            }
        }
    }
    tokens
}

/// Reads the two options from a `localconfig.vdf` text.
pub fn steam_settings(text: &str, account: &str) -> SteamSettings {
    // Steam writes the block only after a Remote Play option was changed;
    // without it both options have Steam's defaults.
    let pairs = vdf_block(text, "streaming_v2").unwrap_or_default();
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    SteamSettings {
        account: account.into(),
        custom_display_device: get("CustomDisplayDevice"),
        enable_capture_nvfbc: get("EnableCaptureNVFBC"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALCONFIG: &str = r#"
"UserLocalConfigStore"
{
	"Software" { "Valve" { "Steam" { "streaming_v2" "not a block" } } }
	"streaming_v2"
	{
		"EnableStreaming"		"1"
		"CustomDisplayDevice"		"SUDOVDA"
		"Nested" { "EnableCaptureNVFBC" "9" }
		"EnableCaptureNVFBC"		"1"
		"Quote"		"a \"b\" c\\"
	}
	"other" { "EnableCaptureNVFBC" "0" }
}
"#;

    fn adapter(version: &str, monitors: &[&str]) -> Device {
        Device {
            instance_id: r"ROOT\DISPLAY\0000".into(),
            started: Some(true),
            driver_version: Some(version.into()),
            monitors: Some(monitors.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    fn level(checks: &[Check], name: &str) -> Level {
        checks.iter().find(|c| c.name == name).unwrap().level
    }

    #[test]
    fn reads_streaming_block() {
        let s = steam_settings(LOCALCONFIG, "only");
        assert_eq!(s.custom_display_device.as_deref(), Some("SUDOVDA"));
        assert_eq!(s.enable_capture_nvfbc.as_deref(), Some("1"));
        let pairs = vdf_block(LOCALCONFIG, "streaming_v2").unwrap();
        assert!(pairs.contains(&("Quote".into(), r#"a "b" c\"#.into())));
        let empty = steam_settings("\"x\" { }", "only");
        assert!(empty.custom_display_device.is_none() && empty.enable_capture_nvfbc.is_none());
    }

    #[test]
    fn steam_rules() {
        let setup = |device: Option<&str>, nvfbc: Option<&str>| Setup {
            steam: Some(SteamSettings {
                account: "only".into(),
                custom_display_device: device.map(Into::into),
                enable_capture_nvfbc: nvfbc.map(Into::into),
            }),
            ..Default::default()
        };
        let c = evaluate(None, &setup(Some("sudovda"), Some("0")));
        assert_eq!(level(&c, "steam_primary_display"), Level::Ok);
        assert_eq!(level(&c, "steam_nvfbc"), Level::Ok);
        let c = evaluate(None, &setup(Some(""), Some("1")));
        assert_eq!(level(&c, "steam_primary_display"), Level::Warn);
        assert_eq!(level(&c, "steam_nvfbc"), Level::Warn);
        let c = evaluate(None, &setup(None, None));
        assert_eq!(level(&c, "steam_nvfbc"), Level::Unknown);
        let c = evaluate(None, &setup(None, None));
        assert_eq!(level(&c, "steam_primary_display"), Level::Warn);
        let c = evaluate(None, &Setup::default());
        assert_eq!(level(&c, "steam_primary_display"), Level::Unknown);
        let none = Setup {
            steam: Some(SteamSettings {
                account: NO_ACCOUNT.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            level(&evaluate(None, &none), "steam_primary_display"),
            Level::Warn
        );
    }

    #[test]
    fn driver_rules() {
        let c = |devices: Option<&[Device]>| driver(devices, Some(Health::Ok)).level;
        assert_eq!(c(Some(&[adapter(TESTED_DRIVER, &[])])), Level::Ok);
        // Adapter fine, certificate missing from a store.
        let ok = [adapter(TESTED_DRIVER, &[])];
        let repair = driver(Some(&ok), Some(Health::Broken));
        assert_eq!(repair.level, Level::Warn);
        assert!(repair.message.contains("urakata driver repair"));
        assert_eq!(driver(Some(&ok), None).level, Level::Unknown);
        let untested = [adapter("1.10.9.300", &[])];
        assert_eq!(driver(Some(&untested), None).level, Level::Warn);
        // A problem on the adapter itself keeps its own message.
        let stopped = Device {
            started: Some(false),
            ..adapter(TESTED_DRIVER, &[])
        };
        let m = driver(Some(&[stopped]), Some(Health::Broken)).message;
        assert_eq!(m, "adapter is not started");
        assert_eq!(c(Some(&[adapter("1.10.9.300", &[])])), Level::Warn);
        assert_eq!(c(Some(&[])), Level::Warn);
        assert_eq!(c(None), Level::Unknown);
        let broken = Device {
            problem_code: Some(43),
            ..adapter(TESTED_DRIVER, &[])
        };
        assert_eq!(c(Some(&[broken])), Level::Warn);
    }

    #[test]
    fn steam_debug_port_rule() {
        assert_eq!(steam_debug_port(Some(false)).level, Level::Ok);
        assert_eq!(steam_debug_port(None).level, Level::Unknown);
        let on = steam_debug_port(Some(true));
        assert_eq!(on.level, Level::Warn);
        assert!(on.message.contains(CEF_DEBUG_FLAG));
        let setup = Setup {
            steam_debug_flag: Some(true),
            ..Default::default()
        };
        assert_eq!(
            level(&evaluate(None, &setup), "steam_debug_port"),
            Level::Warn
        );
    }

    #[test]
    fn watcher_and_layout_rules() {
        let on = Autostart {
            command: Some(r#""%USERPROFILE%\u\urakata-watch.exe""#.into()),
            program_exists: Some(true),
            disabled: Some(false),
        };
        assert_eq!(autostart(Some(&on)).level, Level::Ok);
        let off = Autostart {
            disabled: Some(true),
            ..on
        };
        assert_eq!(autostart(Some(&off)).level, Level::Warn);
        assert_eq!(autostart(Some(&Autostart::default())).level, Level::Warn);
        assert_eq!(watcher(Some(false)).level, Level::Warn);

        let idle = [adapter(TESTED_DRIVER, &[])];
        let stream = [adapter(TESTED_DRIVER, &[r"DISPLAY\SMKD1CE\1&0&UID256"])];
        assert_eq!(session(Some(&idle), Some(true)).level, Level::Warn);
        assert_eq!(session(Some(&stream), Some(true)).level, Level::Ok);
        assert_eq!(session(Some(&idle), Some(false)).level, Level::Ok);
        assert_eq!(session(None, Some(true)).level, Level::Unknown);
    }

    #[test]
    fn profile_is_hidden() {
        let p = Some(r"C:\Users\ab");
        assert_eq!(
            hide_profile(r#""C:\Users\AB\x\urakata-watch.exe""#, p),
            r#""%USERPROFILE%\x\urakata-watch.exe""#
        );
        assert_eq!(hide_profile(r"C:\Users\abc\x", p), r"C:\Users\abc\x");
        assert_eq!(hide_profile(r"C:\Users\ab", p), "%USERPROFILE%");
        assert_eq!(hide_profile(r"D:\x", None), r"D:\x");
        assert_eq!(hide_profile(r"C:\x", Some("\\")), r"C:\x");
        assert_eq!(
            hide_profile(r"C:\Users\ölaf\w.exe", Some(r"C:\Users\Ölaf\")),
            r"%USERPROFILE%\w.exe"
        );
    }

    #[test]
    fn program_from_command() {
        assert_eq!(command_program(r#""C:\a b\w.exe" --x"#), r"C:\a b\w.exe");
        assert_eq!(command_program(r"C:\w.exe --x"), r"C:\w.exe");
    }
}
