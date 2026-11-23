//! SudoVDA driver install, repair and removal.
//!
//! The plan is pure logic and is tested on every platform: it turns the
//! state that the Windows part reads into a list of steps. Only the Windows
//! part (`win`) changes the system.
//!
//! The package is the stock SudoVDA build that Apollo 0.4.6 ships, which
//! the host tests used. Its files are pinned by SHA-256 and the certificate by
//! its SHA-1 thumbprint, so a changed or damaged file is never installed.

use serde::Serialize;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{
    VerifiedPackage, apply, is_elevated, read_health, read_state, sha256, verify_package,
};

/// Driver version of the pinned package (INF `DriverVer`).
pub const PACKAGE_DRIVER_VERSION: &str = "1.10.9.289";

/// INF file name in the package folder.
pub const INF_FILE: &str = "SudoVDA.inf";

/// Certificate file name in the package folder.
pub const CERT_FILE: &str = "sudovda.cer";

/// SHA-1 thumbprint of `CN=sudovda@su.mk`.
pub const CERT_THUMBPRINT: &str = "3C918FC73525AD8B1521B6DB26B71F694277CC49";

/// Package files and their SHA-256 (Apollo 0.4.6, checked against the
/// driver store copy).
pub const PACKAGE_FILES: [(&str, &str); 3] = [
    (
        "SudoVDA.inf",
        "AD69AC682756F0CF339B081FAC7E6E8159FDF2CA01CA69DF8945C7246C286925",
    ),
    (
        "sudovda.cat",
        "2F9189DE5604BEC9D86F51640CC540639E394D9AD0F8E689129375E95F2D22F8",
    ),
    (
        "SudoVDA.dll",
        "47EE263CB5DE9382C6630A2D7F3DAFEC4A49419F953BEEC869CA5DD0C460FF63",
    ),
];

/// Machine certificate stores that must hold the certificate. Root makes
/// Windows trust the chain, TrustedPublisher lets the install run without a
/// publisher prompt (as Apollo's `install.bat`).
pub const CERT_STORES: [&str; 2] = ["Root", "TrustedPublisher"];

/// Upper-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// The INF text (ASCII, UTF-8 or UTF-16 LE with BOM) names the SudoVDA
/// hardware ID. Used to find SudoVDA packages in the driver store.
pub fn inf_matches_sudovda(bytes: &[u8]) -> bool {
    let text = match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let wide: Vec<u16> = rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&wide)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    text.to_ascii_lowercase()
        .contains(&sudovda::HARDWARE_ID.to_ascii_lowercase())
}

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Install what is missing. A working driver is left as it is.
    Install,
    /// Install the package again over the existing device, even when it
    /// works.
    Reinstall,
    /// Fix what is broken: missing certificate, missing or broken device,
    /// extra device nodes.
    Repair,
    /// Remove the device nodes and the driver packages they use.
    Uninstall {
        /// Leave the certificate in the machine stores (for example when
        /// Apollo still needs it).
        keep_certificate: bool,
    },
}

/// One SudoVDA device node (hardware ID `root\sudomaker\sudovda`), present
/// or not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DeviceState {
    /// PnP instance ID, for example `ROOT\DISPLAY\0000`.
    pub instance_id: String,
    /// The node is in the device tree now.
    pub present: bool,
    /// `DN_STARTED` is set.
    pub started: bool,
    /// Problem code (`CM_PROB_*`) when the node has a problem.
    pub problem_code: Option<u32>,
    /// Installed driver version. `None` when no driver is installed.
    pub driver_version: Option<String>,
    /// Published INF name in the driver store, for example `oem122.inf`.
    pub driver_inf: Option<String>,
}

impl DeviceState {
    /// Present, started, no problem and a driver installed.
    pub fn healthy(&self) -> bool {
        self.present && self.started && self.problem_code.is_none() && self.driver_version.is_some()
    }
}

/// Driver state read from the system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct State {
    /// All SudoVDA device nodes.
    pub devices: Vec<DeviceState>,
    /// The certificate is in each of [`CERT_STORES`], same order.
    pub certificate: [bool; 2],
    /// A SudoVDA monitor is present, so a stream probably runs.
    pub stream_active: bool,
    /// Published INFs in the driver store (`oem*.inf`) for the SudoVDA
    /// hardware ID, also ones that no node uses.
    pub packages: Vec<String>,
}

/// Summary for `urakata driver status` and the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// One working device and the certificate in both stores.
    Ok,
    /// No SudoVDA device node.
    Missing,
    /// A device node exists, but something needs a repair.
    Broken,
}

impl State {
    /// The overall health.
    pub fn health(&self) -> Health {
        if self.devices.is_empty() {
            Health::Missing
        } else if self.devices.len() == 1
            && self.devices[0].healthy()
            && self.certificate.iter().all(|&c| c)
        {
            Health::Ok
        } else {
            Health::Broken
        }
    }

    /// The device that `repair` keeps: the first healthy one, else the
    /// first present one, else the first.
    fn keeper(&self) -> Option<usize> {
        let find = |f: fn(&DeviceState) -> bool| self.devices.iter().position(f);
        find(DeviceState::healthy)
            .or_else(|| find(|d| d.present))
            .or(if self.devices.is_empty() {
                None
            } else {
                Some(0)
            })
    }

    /// Distinct driver store INFs: the ones in use in device order, then
    /// the other SudoVDA packages.
    fn driver_infs(&self) -> Vec<String> {
        let mut infs: Vec<String> = Vec::new();
        let used = self.devices.iter().filter_map(|d| d.driver_inf.clone());
        for inf in used.chain(self.packages.iter().cloned()) {
            if !infs.iter().any(|i| i.eq_ignore_ascii_case(&inf)) {
                infs.push(inf);
            }
        }
        infs
    }
}

/// One change to the system, in the order of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Add the package certificate to a machine store.
    AddCertificate {
        /// Store name, one of [`CERT_STORES`].
        store: &'static str,
    },
    /// Create a root device node with the SudoVDA hardware ID.
    CreateDevice,
    /// Install the package INF on the matching devices
    /// (`DiInstallDriverW`).
    InstallDriver {
        /// Install even when the same or a newer driver is installed.
        force: bool,
    },
    /// Remove one device node (`DiUninstallDevice`).
    RemoveDevice {
        /// PnP instance ID.
        instance_id: String,
    },
    /// Delete a driver package from the driver store.
    DeletePackage {
        /// Published INF name, for example `oem122.inf`.
        inf: String,
    },
    /// Remove the package certificate from a machine store.
    RemoveCertificate {
        /// Store name, one of [`CERT_STORES`].
        store: &'static str,
    },
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddCertificate { store } => {
                write!(f, "add the certificate to LocalMachine\\{store}")
            }
            Self::CreateDevice => write!(f, "create the device node {}", sudovda::HARDWARE_ID),
            Self::InstallDriver { force: false } => write!(f, "install the driver"),
            Self::InstallDriver { force: true } => write!(f, "install the driver again"),
            Self::RemoveDevice { instance_id } => write!(f, "remove the device {instance_id}"),
            Self::DeletePackage { inf } => write!(f, "delete the driver package {inf}"),
            Self::RemoveCertificate { store } => {
                write!(f, "remove the certificate from LocalMachine\\{store}")
            }
        }
    }
}

/// Why no plan was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A SudoVDA monitor is present. A driver change restarts the adapter
    /// and ends the stream.
    StreamActive,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StreamActive => {
                f.write_str("a SudoVDA display is active (a stream runs); end the stream first")
            }
        }
    }
}

/// The steps that bring the system from `state` to what `action` asks for.
/// An empty plan means nothing to do.
pub fn plan(action: Action, state: &State) -> Result<Vec<Step>, Refusal> {
    if state.stream_active {
        return Err(Refusal::StreamActive);
    }
    let mut steps = Vec::new();
    let add_certificates = |steps: &mut Vec<Step>| {
        for (store, present) in CERT_STORES.iter().zip(state.certificate) {
            if !present {
                steps.push(Step::AddCertificate { store });
            }
        }
    };
    match action {
        Action::Install => {
            add_certificates(&mut steps);
            // Nodes that are not present cannot start, so only present
            // ones count. A present but broken node is left for `repair`.
            let mut present = state.devices.iter().filter(|d| d.present).peekable();
            if present.peek().is_none() {
                steps.push(Step::CreateDevice);
                steps.push(Step::InstallDriver { force: false });
            } else if !present.any(|d| d.driver_version.is_some()) {
                steps.push(Step::InstallDriver { force: false });
            }
        }
        Action::Reinstall => {
            add_certificates(&mut steps);
            if !state.devices.iter().any(|d| d.present) {
                steps.push(Step::CreateDevice);
            }
            steps.push(Step::InstallDriver { force: true });
        }
        Action::Repair => {
            add_certificates(&mut steps);
            match state.keeper() {
                None => {
                    steps.push(Step::CreateDevice);
                    steps.push(Step::InstallDriver { force: false });
                }
                Some(keep) => {
                    // Extra nodes are removed first, so the install below
                    // reaches only the kept node.
                    for (i, d) in state.devices.iter().enumerate() {
                        if i != keep {
                            steps.push(Step::RemoveDevice {
                                instance_id: d.instance_id.clone(),
                            });
                        }
                    }
                    let kept = &state.devices[keep];
                    if !kept.present {
                        // A root node that is not present cannot start;
                        // make a new one.
                        steps.push(Step::RemoveDevice {
                            instance_id: kept.instance_id.clone(),
                        });
                        steps.push(Step::CreateDevice);
                        steps.push(Step::InstallDriver { force: true });
                    } else if !kept.healthy() {
                        steps.push(Step::InstallDriver { force: true });
                    }
                }
            }
        }
        Action::Uninstall { keep_certificate } => {
            for d in &state.devices {
                steps.push(Step::RemoveDevice {
                    instance_id: d.instance_id.clone(),
                });
            }
            for inf in state.driver_infs() {
                steps.push(Step::DeletePackage { inf });
            }
            if !keep_certificate {
                for (store, present) in CERT_STORES.iter().zip(state.certificate) {
                    if present {
                        steps.push(Step::RemoveCertificate { store });
                    }
                }
            }
        }
    }
    Ok(steps)
}

/// First line of a driver command that had nothing to do. The settings
/// window reads it from the `--log` file.
pub const NOTHING_TO_DO: &str = "Nothing to do.";

/// Line of a driver command after which Windows wants a restart. The
/// settings window reads it from the `--log` file.
pub const RESTART_NOTE: &str = "Windows asks for a restart to finish.";

/// Text for `urakata driver status`.
pub fn status_text(state: &State) -> String {
    let mut out = String::new();
    let health = match state.health() {
        Health::Ok => "ok",
        Health::Missing => "not installed",
        Health::Broken => "needs repair (run `urakata driver repair` as admin)",
    };
    out.push_str(&format!("SudoVDA driver: {health}\n"));
    for d in &state.devices {
        let mut notes = Vec::new();
        notes.push(match &d.driver_version {
            Some(v) => format!("driver {v}"),
            None => "no driver".into(),
        });
        if !d.present {
            notes.push("not present".into());
        } else if !d.started {
            notes.push("not started".into());
        }
        if let Some(p) = d.problem_code {
            notes.push(format!("problem code {p}"));
        }
        out.push_str(&format!(
            "  device {}: {}\n",
            d.instance_id,
            notes.join(", ")
        ));
    }
    for (store, present) in CERT_STORES.iter().zip(state.certificate) {
        let mark = if present { "present" } else { "missing" };
        out.push_str(&format!("  certificate in LocalMachine\\{store}: {mark}\n"));
    }
    if let Some(v) = state
        .devices
        .iter()
        .find_map(|d| d.driver_version.as_deref())
        && v != PACKAGE_DRIVER_VERSION
    {
        out.push_str(&format!(
            "  The installed driver differs from the bundled {PACKAGE_DRIVER_VERSION}.\n"
        ));
    }
    if state.stream_active {
        out.push_str("  A SudoVDA display is active (a stream runs).\n");
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> DeviceState {
        DeviceState {
            instance_id: id.into(),
            present: true,
            started: true,
            problem_code: None,
            driver_version: Some(PACKAGE_DRIVER_VERSION.into()),
            driver_inf: Some("oem122.inf".into()),
        }
    }

    fn healthy() -> State {
        State {
            devices: vec![device(r"ROOT\DISPLAY\0000")],
            certificate: [true, true],
            stream_active: false,
            packages: vec!["oem122.inf".into()],
        }
    }

    #[test]
    fn health() {
        assert_eq!(healthy().health(), Health::Ok);
        assert_eq!(State::default().health(), Health::Missing);
        let mut s = healthy();
        s.certificate[1] = false;
        assert_eq!(s.health(), Health::Broken);
        let mut s = healthy();
        s.devices[0].problem_code = Some(28);
        assert_eq!(s.health(), Health::Broken);
        let mut s = healthy();
        s.devices.push(device(r"ROOT\DISPLAY\0001"));
        assert_eq!(s.health(), Health::Broken);
    }

    #[test]
    fn install_on_empty_system() {
        assert_eq!(
            plan(Action::Install, &State::default()),
            Ok(vec![
                Step::AddCertificate { store: "Root" },
                Step::AddCertificate {
                    store: "TrustedPublisher"
                },
                Step::CreateDevice,
                Step::InstallDriver { force: false },
            ])
        );
    }

    #[test]
    fn install_leaves_working_driver() {
        assert_eq!(plan(Action::Install, &healthy()), Ok(vec![]));
        // An other driver version is kept too; reinstall replaces it.
        let mut s = healthy();
        s.devices[0].driver_version = Some("1.9.0.0".into());
        assert_eq!(plan(Action::Install, &s), Ok(vec![]));
    }

    #[test]
    fn install_on_node_without_driver() {
        let mut s = healthy();
        s.devices[0].driver_version = None;
        s.devices[0].driver_inf = None;
        assert_eq!(
            plan(Action::Install, &s),
            Ok(vec![Step::InstallDriver { force: false }])
        );
    }

    #[test]
    fn install_ignores_absent_nodes() {
        let mut s = healthy();
        s.devices[0].present = false;
        s.devices[0].started = false;
        assert_eq!(
            plan(Action::Install, &s),
            Ok(vec![
                Step::CreateDevice,
                Step::InstallDriver { force: false }
            ])
        );
        assert_eq!(
            plan(Action::Reinstall, &s),
            Ok(vec![
                Step::CreateDevice,
                Step::InstallDriver { force: true }
            ])
        );
    }

    #[test]
    fn install_leaves_broken_node_to_repair() {
        let mut s = healthy();
        s.devices[0].problem_code = Some(10);
        assert_eq!(plan(Action::Install, &s), Ok(vec![]));
        assert_eq!(s.health(), Health::Broken);
    }

    #[test]
    fn uninstall_deletes_unused_packages() {
        let s = State {
            packages: vec!["oem130.inf".into()],
            ..State::default()
        };
        assert_eq!(
            plan(
                Action::Uninstall {
                    keep_certificate: true
                },
                &s
            ),
            Ok(vec![Step::DeletePackage {
                inf: "oem130.inf".into()
            }])
        );
    }

    #[test]
    fn reinstall_forces() {
        assert_eq!(
            plan(Action::Reinstall, &healthy()),
            Ok(vec![Step::InstallDriver { force: true }])
        );
        assert_eq!(
            plan(Action::Reinstall, &State::default()).unwrap()[2..],
            [Step::CreateDevice, Step::InstallDriver { force: true }]
        );
    }

    #[test]
    fn repair_healthy_is_empty() {
        assert_eq!(plan(Action::Repair, &healthy()), Ok(vec![]));
    }

    #[test]
    fn repair_problem_and_certificate() {
        let mut s = healthy();
        s.devices[0].problem_code = Some(10);
        s.certificate[0] = false;
        assert_eq!(
            plan(Action::Repair, &s),
            Ok(vec![
                Step::AddCertificate { store: "Root" },
                Step::InstallDriver { force: true },
            ])
        );
    }

    #[test]
    fn repair_removes_extra_nodes_and_keeps_healthy_one() {
        let mut s = healthy();
        let mut broken = device(r"ROOT\DISPLAY\0001");
        broken.problem_code = Some(10);
        s.devices.insert(0, broken);
        assert_eq!(
            plan(Action::Repair, &s),
            Ok(vec![Step::RemoveDevice {
                instance_id: r"ROOT\DISPLAY\0001".into()
            }])
        );
    }

    #[test]
    fn repair_replaces_absent_node() {
        let mut s = healthy();
        s.devices[0].present = false;
        s.devices[0].started = false;
        assert_eq!(
            plan(Action::Repair, &s),
            Ok(vec![
                Step::RemoveDevice {
                    instance_id: r"ROOT\DISPLAY\0000".into()
                },
                Step::CreateDevice,
                Step::InstallDriver { force: true },
            ])
        );
    }

    #[test]
    fn uninstall() {
        let mut s = healthy();
        s.devices.push(device(r"ROOT\DISPLAY\0001"));
        let steps = plan(
            Action::Uninstall {
                keep_certificate: false,
            },
            &s,
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::RemoveDevice {
                    instance_id: r"ROOT\DISPLAY\0000".into()
                },
                Step::RemoveDevice {
                    instance_id: r"ROOT\DISPLAY\0001".into()
                },
                Step::DeletePackage {
                    inf: "oem122.inf".into()
                },
                Step::RemoveCertificate { store: "Root" },
                Step::RemoveCertificate {
                    store: "TrustedPublisher"
                },
            ]
        );
        let kept = plan(
            Action::Uninstall {
                keep_certificate: true,
            },
            &s,
        )
        .unwrap();
        assert_eq!(kept, steps[..3]);
    }

    #[test]
    fn refuses_during_stream() {
        let mut s = healthy();
        s.stream_active = true;
        for action in [
            Action::Install,
            Action::Reinstall,
            Action::Repair,
            Action::Uninstall {
                keep_certificate: true,
            },
        ] {
            assert_eq!(plan(action, &s), Err(Refusal::StreamActive));
        }
    }

    #[test]
    fn status_text_lists_parts() {
        let text = status_text(&healthy());
        assert!(text.starts_with("SudoVDA driver: ok"));
        assert!(text.contains(r"device ROOT\DISPLAY\0000: driver 1.10.9.289"));
        assert!(!text.contains("differs"));
        let mut s = healthy();
        s.devices[0].driver_version = Some("1.9.0.0".into());
        s.certificate[1] = false;
        let text = status_text(&s);
        assert!(text.contains("needs repair"));
        assert!(text.contains("TrustedPublisher: missing"));
        assert!(text.contains("differs from the bundled"));
    }

    #[test]
    fn finds_sudovda_infs() {
        let ascii =
            b"[Standard.NTamd64]\r\n%DeviceName%=SudoVDA_Install, Root\\SudoMaker\\SudoVDA\r\n";
        assert!(inf_matches_sudovda(ascii));
        let mut utf16 = vec![0xFF, 0xFE];
        for w in "x, Root\\SudoMaker\\SudoVDA".encode_utf16() {
            utf16.extend_from_slice(&w.to_le_bytes());
        }
        assert!(inf_matches_sudovda(&utf16));
        assert!(!inf_matches_sudovda(b"PCI\\VEN_10DE&DEV_2B85"));
    }

    #[test]
    fn pinned_values_are_well_formed() {
        assert_eq!(CERT_THUMBPRINT.len(), 40);
        for (_, sha) in PACKAGE_FILES {
            assert_eq!(sha.len(), 64);
            assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert!(PACKAGE_FILES.iter().any(|(n, _)| *n == INF_FILE));
        assert_eq!(hex(&[0x0a, 0xff]), "0AFF");
    }
}
