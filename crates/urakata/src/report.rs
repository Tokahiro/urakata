//! The status report: plain data, JSON form and text form.
//!
//! Collection is Windows only (`status.rs`). The types and the rules here
//! build on every platform, so the tests run anywhere.

#![cfg_attr(not(windows), allow(dead_code))]

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::checks::{Check, Level, Setup};

/// Everything `urakata status` reports. A section or value that could not
/// be read is `null` and its reason is in `errors`.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// Present device nodes with the SudoVDA hardware ID. `None` when the
    /// device list could not be read.
    pub devices: Option<Vec<Device>>,
    /// The control interface, if a handle could be opened.
    pub control: Option<Control>,
    /// `HKLM\SOFTWARE\SudoMaker\SudoVDA`.
    pub registry: Option<Registry>,
    /// Active display paths (CCD `QDC_ONLY_ACTIVE_PATHS`).
    pub displays: Option<Vec<Display>>,
    /// Facts about Steam and the watcher that the checks use.
    pub setup: Setup,
    /// Setup checks (`checks.rs`). A warning does not change the exit code.
    pub checks: Vec<Check>,
    /// Problems met while collecting. Empty when all sections were read.
    pub errors: Vec<String>,
}

/// A SudoVDA adapter device node.
#[derive(Debug, Default, Serialize)]
pub struct Device {
    /// PnP instance ID, for example `ROOT\DISPLAY\0000`.
    pub instance_id: String,
    /// `DEVPKEY_Device_DeviceDesc`.
    pub description: Option<String>,
    /// `DN_STARTED` is set. `None` when the node status could not be read.
    pub started: Option<bool>,
    /// Problem code (`CM_PROB_*`) when `DN_HAS_PROBLEM` is set.
    pub problem_code: Option<u32>,
    /// `DEVPKEY_Device_DriverVersion` (the INF `DriverVer`).
    pub driver_version: Option<String>,
    /// `DEVPKEY_Device_DriverProvider`.
    pub driver_provider: Option<String>,
    /// `DEVPKEY_Device_DriverInfPath`, for example `oem122.inf`.
    pub driver_inf: Option<String>,
    /// Instance IDs of present child monitors (the virtual displays).
    /// `None` when the list could not be read.
    pub monitors: Option<Vec<String>>,
}

/// State read through the control interface.
#[derive(Debug, Default, Serialize)]
pub struct Control {
    /// Device interface path that was opened.
    pub path: String,
    /// `IOCTL_GET_WATCHDOG`. Does not reset the watchdog.
    pub watchdog: Option<Watchdog>,
    /// `IOCTL_GET_PROTOCOL_VERSION`. Read only while no virtual display
    /// exists, because it resets the watchdog.
    pub protocol_version: Option<ProtocolVersion>,
    /// Why the protocol version was not read.
    pub protocol_version_skipped: Option<String>,
}

/// Watchdog state in seconds.
#[derive(Debug, Default, Serialize)]
pub struct Watchdog {
    /// Timeout. 0 means off.
    pub timeout_s: u32,
    /// Seconds left before the driver removes all virtual displays.
    pub countdown_s: u32,
}

/// Protocol version reported by the driver.
#[derive(Debug, Default, Serialize)]
pub struct ProtocolVersion {
    /// Major version.
    pub major: u8,
    /// Minor version.
    pub minor: u8,
    /// Incremental version.
    pub incremental: u8,
    /// The driver reports a test build.
    pub test_build: bool,
}

impl From<sudovda::ProtocolVersion> for ProtocolVersion {
    fn from(v: sudovda::ProtocolVersion) -> Self {
        Self {
            major: v.major,
            minor: v.minor,
            incremental: v.incremental,
            test_build: v.is_test_build(),
        }
    }
}

/// The driver's registry settings. A value that is not set is `null`: the
/// driver then uses its built-in default.
#[derive(Debug, Default, Serialize)]
pub struct Registry {
    /// The key exists.
    pub key_present: bool,
    /// Values under the names the driver reads (`LoadSettings`).
    pub values: RegistryValues,
}

/// Values of `HKLM\SOFTWARE\SudoMaker\SudoVDA`. JSON names are the
/// registry value names.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryValues {
    /// `gpuName` (string).
    pub gpu_name: Option<String>,
    /// `testMode` (DWORD).
    pub test_mode: Option<u32>,
    /// `watchdog` in seconds (DWORD).
    pub watchdog: Option<u32>,
    /// `maxMonitors` (DWORD).
    pub max_monitors: Option<u32>,
    /// `sdrBits` (DWORD).
    pub sdr_bits: Option<u32>,
    /// `hdrBits` (DWORD).
    pub hdr_bits: Option<u32>,
}

/// One active display path.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Display {
    /// GDI name of the source, for example `\\.\DISPLAY1`.
    pub gdi_name: String,
    /// Monitor name from the EDID.
    pub monitor_name: String,
    /// Monitor device interface path.
    pub monitor_path: String,
    /// The monitor is a child of a SudoVDA adapter.
    pub sudovda: bool,
    /// The source is at desktop position (0, 0), which the CCD API defines
    /// as the primary.
    pub primary: bool,
    /// Desktop position, left edge.
    pub x: i32,
    /// Desktop position, top edge.
    pub y: i32,
    /// Source width in pixels.
    pub width: u32,
    /// Source height in pixels.
    pub height: u32,
    /// Target refresh rate in Hz.
    pub refresh_hz: Option<f64>,
    /// `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`.
    pub advanced_color: Option<AdvancedColor>,
}

/// Advanced color state of a target. "Advanced color" is the CCD term; it
/// covers HDR. Whether `enabled` can also mean a non-HDR mode (for example
/// automatic color management) was not checked against a primary source.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AdvancedColor {
    /// `advancedColorSupported`.
    pub supported: bool,
    /// `advancedColorEnabled`.
    pub enabled: bool,
    /// `bitsPerColorChannel`.
    pub bits_per_color_channel: u32,
    /// `wideColorEnforced`: advanced color is on for color management on
    /// an SDR display, not for HDR.
    #[serde(default)]
    pub wide_color_enforced: bool,
}

/// Decides whether the protocol version may be read.
///
/// The query resets the driver's watchdog, so it runs
/// only when every SudoVDA adapter is known to have no virtual display. An
/// unknown monitor list counts as "maybe present". `devices` is `None` when
/// the adapter list was not read in full.
pub fn protocol_gate(devices: Option<&[Device]>) -> Result<(), String> {
    let Some(devices) = devices else {
        return Err("SudoVDA adapter list not read in full, virtual displays unknown".into());
    };
    if devices.is_empty() {
        return Err("no SudoVDA adapter found, virtual displays unknown".into());
    }
    let mut count = 0;
    for d in devices {
        match &d.monitors {
            None => return Err("virtual display list could not be read".into()),
            Some(m) => count += m.len(),
        }
    }
    match count {
        0 => Ok(()),
        1 => Err("1 virtual display present".into()),
        n => Err(format!("{n} virtual displays present")),
    }
}

/// True when the monitor interface path belongs to one of the monitor
/// device nodes. An interface path is `\\?\`, the instance ID with `\`
/// replaced by `#`, then `#` and the interface GUID, for example
/// `\\?\DISPLAY#SMKD1CE#1&28a6823a&0&UID256#{...}` for
/// `DISPLAY\SMKD1CE\1&28A6823A&0&UID256`.
pub fn path_matches_monitor(path: &str, instance_ids: &[String]) -> bool {
    let path = path.to_ascii_uppercase();
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return false;
    };
    instance_ids.iter().any(|id| {
        let id = id.replace('\\', "#").to_ascii_uppercase();
        rest.starts_with(&format!("{id}#"))
    })
}

fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref().map_or_else(|| "-".to_string(), T::to_string)
}

impl Report {
    /// Human-readable form.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        // Writing to a String cannot fail.
        let mut line = |text: String| {
            let _ = writeln!(s, "{text}");
        };

        line("SudoVDA adapter".into());
        match &self.devices {
            None => line("  not read".into()),
            Some(list) if list.is_empty() => line("  not found".into()),
            Some(_) => {}
        }
        for d in self.devices.iter().flatten() {
            line(format!(
                "  {} ({})",
                d.instance_id,
                d.description.as_deref().unwrap_or("-")
            ));
            let state = match (d.started, d.problem_code) {
                (_, Some(p)) => format!("problem code {p}"),
                (Some(true), None) => "started".into(),
                (Some(false), None) => "not started".into(),
                (None, None) => "unknown".into(),
            };
            line(format!("  state      {state}"));
            line(format!(
                "  driver     {} ({}, {})",
                opt(&d.driver_version),
                opt(&d.driver_provider),
                opt(&d.driver_inf)
            ));
            match &d.monitors {
                None => line("  monitors   unknown".into()),
                Some(m) if m.is_empty() => line("  monitors   none".into()),
                Some(m) => {
                    for id in m {
                        line(format!("  monitor    {id}"));
                    }
                }
            }
        }

        line("Control interface".into());
        match &self.control {
            None => line("  not opened".into()),
            Some(c) => {
                line(format!("  path       {}", c.path));
                let watchdog = c.watchdog.as_ref().map_or("-".into(), |w| {
                    format!("timeout {} s, countdown {} s", w.timeout_s, w.countdown_s)
                });
                line(format!("  watchdog   {watchdog}"));
                let protocol = match (&c.protocol_version, &c.protocol_version_skipped) {
                    (Some(v), _) => format!(
                        "{}.{}.{}{}",
                        v.major,
                        v.minor,
                        v.incremental,
                        if v.test_build { " (test build)" } else { "" }
                    ),
                    (None, Some(why)) => format!("not read: {why}"),
                    (None, None) => "-".into(),
                };
                line(format!("  protocol   {protocol}"));
            }
        }

        line(format!("Registry HKLM\\{}", sudovda::REGISTRY_KEY));
        match &self.registry {
            None => line("  not read".into()),
            Some(r) if !r.key_present => line("  key not present (driver defaults)".into()),
            Some(r) => {
                let v = &r.values;
                line(format!("  gpuName     {}", opt(&v.gpu_name)));
                line(format!("  testMode    {}", opt(&v.test_mode)));
                line(format!("  watchdog    {}", opt(&v.watchdog)));
                line(format!("  maxMonitors {}", opt(&v.max_monitors)));
                line(format!("  sdrBits     {}", opt(&v.sdr_bits)));
                line(format!("  hdrBits     {}", opt(&v.hdr_bits)));
            }
        }

        line("Active displays".into());
        match &self.displays {
            None => line("  not read".into()),
            Some(list) if list.is_empty() => line("  none".into()),
            Some(list) => {
                for d in list {
                    let refresh = d.refresh_hz.map_or("-".into(), |hz| format!("{hz:.2} Hz"));
                    let color = d.advanced_color.as_ref().map_or("-".into(), |c| {
                        format!(
                            "advanced color {}, {} bpc",
                            if c.enabled {
                                "on"
                            } else if c.supported {
                                "off"
                            } else {
                                "unsupported"
                            },
                            c.bits_per_color_channel
                        )
                    });
                    let mut tags = String::new();
                    if d.primary {
                        tags.push_str(" primary");
                    }
                    if d.sudovda {
                        tags.push_str(" sudovda");
                    }
                    line(format!(
                        "  {} \"{}\" {}x{} at {},{} {refresh}, {color}{tags}",
                        d.gdi_name, d.monitor_name, d.width, d.height, d.x, d.y
                    ));
                }
            }
        }

        if !self.checks.is_empty() {
            line("Setup checks".into());
            for c in &self.checks {
                let tag = match c.level {
                    Level::Ok => "ok  ",
                    Level::Warn => "WARN",
                    Level::Unknown => "?   ",
                };
                line(format!("  {tag} {}: {}", c.name, c.message));
            }
        }

        if !self.errors.is_empty() {
            line("Errors".into());
            for e in &self.errors {
                line(format!("  {e}"));
            }
        }
        s.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(monitors: Option<&[&str]>) -> Device {
        Device {
            instance_id: r"ROOT\DISPLAY\0000".into(),
            monitors: monitors.map(|m| m.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn gate_reads_only_without_virtual_displays() {
        assert_eq!(protocol_gate(Some(&[device(Some(&[]))])), Ok(()));
        assert!(protocol_gate(None).is_err());
        assert!(protocol_gate(Some(&[])).is_err());
        assert!(protocol_gate(Some(&[device(None)])).is_err());
        assert_eq!(
            protocol_gate(Some(&[device(Some(&[
                r"DISPLAY\SMKD1CE\1&28A6823A&0&UID256"
            ]))])),
            Err("1 virtual display present".into())
        );
        assert!(protocol_gate(Some(&[device(Some(&[])), device(None)])).is_err());
    }

    #[test]
    fn monitor_path_matching() {
        let ids = vec![r"DISPLAY\SMKD1CE\1&28A6823A&0&UID256".to_string()];
        let path =
            r"\\?\DISPLAY#SMKD1CE#1&28a6823a&0&UID256#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert!(path_matches_monitor(path, &ids));
        let other =
            r"\\?\DISPLAY#SMKD1CE#1&28a6823a&0&UID2567#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert!(!path_matches_monitor(other, &ids));
        assert!(!path_matches_monitor(path, &[]));
    }

    fn sample() -> Report {
        Report {
            devices: Some(vec![Device {
                description: Some("SudoMaker Virtual Display Adapter".into()),
                started: Some(true),
                driver_version: Some("1.10.9.289".into()),
                ..device(Some(&[]))
            }]),
            control: Some(Control {
                path: r"\\?\ROOT#DISPLAY#0000#{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}".into(),
                watchdog: Some(Watchdog {
                    timeout_s: 3,
                    countdown_s: 3,
                }),
                protocol_version: Some(sudovda::SOURCE_PROTOCOL_VERSION.into()),
                protocol_version_skipped: None,
            }),
            registry: Some(Registry::default()),
            displays: Some(vec![Display {
                gdi_name: r"\\.\DISPLAY1".into(),
                monitor_name: "Philips".into(),
                primary: true,
                width: 3840,
                height: 2160,
                refresh_hz: Some(144.0),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn json_shape() {
        let v = serde_json::to_value(sample()).unwrap();
        assert_eq!(v["devices"][0]["driver_version"], "1.10.9.289");
        assert_eq!(v["control"]["watchdog"]["timeout_s"], 3);
        assert_eq!(v["control"]["protocol_version"]["test_build"], true);
        assert_eq!(v["registry"]["key_present"], false);
        assert!(v["registry"]["values"]["maxMonitors"].is_null());
        assert_eq!(v["displays"][0]["primary"], true);
        assert!(v["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn text_form() {
        let text = sample().to_text();
        assert!(text.contains("state      started"));
        assert!(text.contains("protocol   0.2.1 (test build)"));
        assert!(text.contains("key not present (driver defaults)"));
        assert!(text.contains(r#"\\.\DISPLAY1 "Philips" 3840x2160 at 0,0 144.00 Hz, - primary"#));
        assert!(!text.contains("Errors"));

        let mut skipped = sample();
        let control = skipped.control.as_mut().unwrap();
        control.protocol_version = None;
        control.protocol_version_skipped = Some("1 virtual display present".into());
        skipped.errors.push("boom".into());
        let text = skipped.to_text();
        assert!(text.contains("protocol   not read: 1 virtual display present"));
        assert!(text.ends_with("Errors\n  boom"));
    }

    #[test]
    fn checks_in_both_forms() {
        let mut r = sample();
        r.setup.driver_health = Some(crate::driver::Health::Ok);
        r.checks = crate::checks::evaluate(r.devices.as_deref(), &r.setup);
        let text = r.to_text();
        assert!(text.contains("Setup checks\n  ok   sudovda_driver: driver 1.10.9.289"));
        assert!(text.contains("  ?    steam_nvfbc: Steam settings not read"));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["checks"][0]["name"], "sudovda_driver");
        assert_eq!(v["checks"][0]["level"], "ok");
        assert!(v["setup"]["steam"].is_null());
    }

    #[test]
    fn text_tells_unread_from_missing_adapter() {
        let mut r = sample();
        r.devices = None;
        assert!(r.to_text().starts_with("SudoVDA adapter\n  not read\n"));
        assert!(serde_json::to_value(&r).unwrap()["devices"].is_null());
        r.devices = Some(vec![]);
        assert!(r.to_text().starts_with("SudoVDA adapter\n  not found\n"));
        assert!(serde_json::to_value(&r).unwrap()["devices"].is_array());

        let mut unknown = sample();
        unknown.devices.as_mut().unwrap()[0].started = None;
        assert!(unknown.to_text().contains("state      unknown"));
    }
}
