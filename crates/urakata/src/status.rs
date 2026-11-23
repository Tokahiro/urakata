//! Collects the status report (Windows only). Read-only: no request here
//! changes the driver, the device or the display setup.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVNODE_STATUS_FLAGS, CM_GETIDLIST_FILTER_CLASS, CM_GETIDLIST_FILTER_PRESENT, CM_Get_Child,
    CM_Get_DevNode_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_ID_List_SizeW,
    CM_Get_Device_ID_ListW, CM_Get_Device_IDW, CM_Get_Sibling, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, CM_PROB, CONFIGRET, CR_BUFFER_SMALL, CR_NO_SUCH_DEVNODE, CR_NO_SUCH_VALUE,
    CR_SUCCESS, DN_HAS_PROBLEM, DN_STARTED, MAX_DEVICE_ID_LEN,
};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_DriverInfPath, DEVPKEY_Device_DriverProvider,
    DEVPKEY_Device_DriverVersion, DEVPKEY_Device_HardwareIds, DEVPROPTYPE,
};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::{HRESULT, PCWSTR};

use crate::ccd::{active_paths, device_info, from_wide, header, luid};
use crate::checks::{self, Autostart, Setup, SteamSettings};
use crate::report::{
    AdvancedColor, Control, Device, Display, Registry, RegistryValues, Report, Watchdog,
    path_matches_monitor, protocol_gate,
};

/// Builds the full report. Each section fails on its own; its reason goes
/// to `errors` and the other sections are still read.
pub fn collect() -> Report {
    let mut report = Report::default();
    // The gate may trust the list only when every Display-class node was
    // checked for the SudoVDA hardware ID.
    let mut complete = false;
    match sudovda_devices(&mut report.errors) {
        Ok((devices, all_checked)) => {
            report.devices = Some(devices);
            complete = all_checked;
        }
        Err(e) => report.errors.push(format!("device list: {e}")),
    }
    let gate_devices = report.devices.as_deref().filter(|_| complete);
    report.control = control(gate_devices, &mut report.errors);
    match registry(&mut report.errors) {
        Ok(r) => report.registry = Some(r),
        Err(e) => report.errors.push(format!("registry: {e}")),
    }
    let monitor_ids: Vec<String> = report
        .devices
        .iter()
        .flatten()
        .flat_map(|d| d.monitors.iter().flatten().cloned())
        .collect();
    match displays(&monitor_ids, &mut report.errors) {
        Ok(list) => report.displays = Some(list),
        Err(e) => report.errors.push(format!("display topology: {e}")),
    }
    report.setup = setup(&mut report.errors);
    report.checks = checks::evaluate(report.devices.as_deref(), &report.setup);
    report
}

/// Instance IDs of all present SudoVDA monitors. Fails when the list may be
/// incomplete, because a caller that changes the display setup must not
/// mistake a virtual display for a physical one.
pub fn sudovda_monitor_ids() -> Result<Vec<String>, String> {
    let mut errors = Vec::new();
    let (devices, all_checked) = sudovda_devices(&mut errors)?;
    if !all_checked {
        return Err(format!(
            "SudoVDA adapter list incomplete: {}",
            errors.join("; ")
        ));
    }
    let mut ids = Vec::new();
    for d in devices {
        match d.monitors {
            Some(m) => ids.extend(m),
            None => return Err(format!("{}: monitor list not read", d.instance_id)),
        }
    }
    Ok(ids)
}

// ---- Control interface -------------------------------------------------

fn control(devices: Option<&[Device]>, errors: &mut Vec<String>) -> Option<Control> {
    // No data access: the queries below use FILE_ANY_ACCESS IOCTLs.
    let device = match sudovda::Device::open_read_only() {
        Ok(d) => d,
        Err(e) => {
            errors.push(format!("control interface: {e}"));
            return None;
        }
    };
    let mut control = Control {
        path: device.path().to_string(),
        ..Default::default()
    };
    // GET_WATCHDOG does not reset the countdown (Driver.cpp), so it is safe
    // during a stream.
    match device.watchdog() {
        Ok(w) => {
            control.watchdog = Some(Watchdog {
                timeout_s: w.timeout,
                countdown_s: w.countdown,
            })
        }
        Err(e) => errors.push(format!("watchdog: {e}")),
    }
    // A virtual display can still appear between the check and the query.
    // Then the query resets the watchdog once, as any client ping does.
    match protocol_gate(devices) {
        Ok(()) => match device.protocol_version() {
            Ok(v) => control.protocol_version = Some(v.into()),
            Err(e) => errors.push(format!("protocol version: {e}")),
        },
        Err(why) => control.protocol_version_skipped = Some(why),
    }
    Some(control)
}

// ---- PnP device nodes --------------------------------------------------

fn cm_error(what: &str, cr: CONFIGRET) -> String {
    format!("{what} failed (CONFIGRET 0x{:x})", cr.0)
}

/// Present device nodes of the Display class whose hardware IDs contain
/// the SudoVDA hardware ID. A failure on one node goes to `errors` and the
/// other nodes are still read. The flag is false when a node could not be
/// checked, so the list can miss an adapter.
fn sudovda_devices(errors: &mut Vec<String>) -> Result<(Vec<Device>, bool), String> {
    let mut devices = Vec::new();
    let mut all_checked = true;
    for id in present_display_class_ids()? {
        let mut devinst = 0u32;
        let w = sudovda::wide(&id);
        // SAFETY: valid out pointer, NUL-terminated ID.
        let cr = unsafe {
            CM_Locate_DevNodeW(&mut devinst, PCWSTR(w.as_ptr()), CM_LOCATE_DEVNODE_NORMAL)
        };
        if cr != CR_SUCCESS {
            // The device left between the list and the lookup.
            continue;
        }
        let hardware_ids = match strings(devinst, &DEVPKEY_Device_HardwareIds) {
            Ok(h) => h,
            Err(e) => {
                errors.push(format!("{id}: hardware IDs: {e}"));
                all_checked = false;
                continue;
            }
        };
        if hardware_ids
            .iter()
            .any(|h| h.eq_ignore_ascii_case(sudovda::HARDWARE_ID))
        {
            devices.push(device(devinst, id, errors));
        }
    }
    Ok((devices, all_checked))
}

fn present_display_class_ids() -> Result<Vec<String>, String> {
    let filter = sudovda::wide(&sudovda::CLASS_GUID.to_string());
    let flags = CM_GETIDLIST_FILTER_CLASS | CM_GETIDLIST_FILTER_PRESENT;
    sudovda::read_cm_list(
        // SAFETY: valid out pointer, NUL-terminated filter.
        |len| unsafe { CM_Get_Device_ID_List_SizeW(len, PCWSTR(filter.as_ptr()), flags) },
        // SAFETY: `buf` is a live, writable buffer.
        |buf| unsafe { CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), buf, flags) },
    )
    .map_err(|e| format!("CM_Get_Device_ID_List: {e}"))
}

/// Reads one adapter node. A value that cannot be read is `None` and its
/// reason goes to `errors`.
fn device(devinst: u32, instance_id: String, errors: &mut Vec<String>) -> Device {
    let mut status = CM_DEVNODE_STATUS_FLAGS::default();
    let mut problem = CM_PROB::default();
    // SAFETY: valid out pointers.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    let (started, problem_code) = if cr == CR_SUCCESS {
        (
            Some(status.0 & DN_STARTED.0 != 0),
            (status.0 & DN_HAS_PROBLEM.0 != 0).then_some(problem.0),
        )
    } else {
        errors.push(format!(
            "{instance_id}: {}",
            cm_error("CM_Get_DevNode_Status", cr)
        ));
        (None, None)
    };
    let mut first = |key, name: &str| match strings(devinst, key) {
        Ok(v) => v.into_iter().next(),
        Err(e) => {
            errors.push(format!("{instance_id}: {name}: {e}"));
            None
        }
    };
    let description = first(&DEVPKEY_Device_DeviceDesc, "description");
    let driver_version = first(&DEVPKEY_Device_DriverVersion, "driver version");
    let driver_provider = first(&DEVPKEY_Device_DriverProvider, "driver provider");
    let driver_inf = first(&DEVPKEY_Device_DriverInfPath, "driver INF");
    let monitors = children(devinst)
        .map_err(|e| errors.push(format!("{instance_id}: child monitors: {e}")))
        .ok();
    Device {
        instance_id,
        description,
        started,
        problem_code,
        driver_version,
        driver_provider,
        driver_inf,
        monitors,
    }
}

/// Reads a string or string-list property. A missing property is empty.
fn strings(devinst: u32, key: &DEVPROPKEY) -> Result<Vec<String>, String> {
    // The property can grow between the size and the data query; then the
    // data query returns CR_BUFFER_SMALL and the size is read again.
    loop {
        let mut ty = DEVPROPTYPE::default();
        let mut size = 0u32;
        // SAFETY: size query with no buffer; valid out pointers.
        let cr = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut ty, None, &mut size, 0) };
        if cr == CR_NO_SUCH_VALUE {
            return Ok(Vec::new());
        }
        if cr != CR_BUFFER_SMALL {
            return Err(cm_error("CM_Get_DevNode_Property", cr));
        }
        let mut buf = vec![0u8; size as usize];
        // SAFETY: `buf` holds `size` bytes.
        let cr = unsafe {
            CM_Get_DevNode_PropertyW(devinst, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0)
        };
        if cr == CR_BUFFER_SMALL {
            continue;
        }
        if cr == CR_NO_SUCH_VALUE {
            return Ok(Vec::new());
        }
        if cr != CR_SUCCESS {
            return Err(cm_error("CM_Get_DevNode_Property", cr));
        }
        buf.truncate(size as usize);
        let utf16: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return Ok(sudovda::split_multi_sz(&utf16));
    }
}

/// Instance IDs of the present child device nodes. SudoVDA monitors are
/// children of the adapter: on the host, `DEVPKEY_Device_Parent` of
/// `DISPLAY\SMKD1CE\1&28A6823A&0&UID256` is `ROOT\DISPLAY\0000`.
fn children(devinst: u32) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut node = 0u32;
    // SAFETY: valid out pointer.
    let mut cr = unsafe { CM_Get_Child(&mut node, devinst, 0) };
    while cr == CR_SUCCESS {
        let mut buf = vec![0u16; MAX_DEVICE_ID_LEN as usize + 1];
        // SAFETY: `buf` is a live, writable buffer.
        let id_cr = unsafe { CM_Get_Device_IDW(node, &mut buf, 0) };
        if id_cr != CR_SUCCESS {
            return Err(cm_error("CM_Get_Device_ID", id_cr));
        }
        ids.extend(sudovda::split_multi_sz(&buf));
        let current = node;
        // SAFETY: valid out pointer.
        cr = unsafe { CM_Get_Sibling(&mut node, current, 0) };
    }
    if cr != CR_NO_SUCH_DEVNODE {
        return Err(cm_error("CM_Get_Child/CM_Get_Sibling", cr));
    }
    Ok(ids)
}

// ---- Registry ----------------------------------------------------------

/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND): missing key or value.
const NOT_FOUND: HRESULT = HRESULT(0x8007_0002_u32 as i32);

/// Maps a missing registry value to `None`.
fn missing<T>(r: windows::core::Result<T>, name: &str) -> Result<Option<T>, String> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(e) => Err(format!("{name}: {e}")),
    }
}

/// Reads the key. A value that cannot be read is `None` and its reason goes
/// to `errors`; the other values are still read.
fn registry(errors: &mut Vec<String>) -> Result<Registry, String> {
    let key = match windows_registry::LOCAL_MACHINE.open(sudovda::REGISTRY_KEY) {
        Ok(k) => k,
        Err(e) if e.code() == NOT_FOUND => return Ok(Registry::default()),
        Err(e) => return Err(e.to_string()),
    };
    let dword = |name: &str| -> Result<Option<u32>, String> {
        match missing(key.get_type(name), name)? {
            None => Ok(None),
            Some(windows_registry::Type::U32) => missing(key.get_u32(name), name),
            Some(other) => Err(format!("{name}: expected REG_DWORD, found {other:?}")),
        }
    };
    let string = |name: &str| -> Result<Option<String>, String> {
        match missing(key.get_type(name), name)? {
            None => Ok(None),
            Some(windows_registry::Type::String) => missing(key.get_string(name), name),
            Some(other) => Err(format!("{name}: expected REG_SZ, found {other:?}")),
        }
    };
    Ok(Registry {
        key_present: true,
        values: RegistryValues {
            gpu_name: keep(string("gpuName"), errors),
            test_mode: keep(dword("testMode"), errors),
            watchdog: keep(dword("watchdog"), errors),
            max_monitors: keep(dword("maxMonitors"), errors),
            sdr_bits: keep(dword("sdrBits"), errors),
            hdr_bits: keep(dword("hdrBits"), errors),
        },
    })
}

/// Moves a per-value error to `errors`; the value is then `None`.
fn keep<T>(r: Result<Option<T>, String>, errors: &mut Vec<String>) -> Option<T> {
    r.unwrap_or_else(|e| {
        errors.push(format!("registry: {e}"));
        None
    })
}

// ---- Display topology (CCD) --------------------------------------------

fn displays(monitor_ids: &[String], errors: &mut Vec<String>) -> Result<Vec<Display>, String> {
    let (paths, modes) = active_paths()?;
    Ok(displays_from(&paths, &modes, monitor_ids, errors))
}

/// One `Display` per path, in the order of `paths`. A value that cannot be
/// read stays at its default and its reason goes to `errors`.
pub fn displays_from(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
    monitor_ids: &[String],
    errors: &mut Vec<String>,
) -> Vec<Display> {
    let mut list = Vec::with_capacity(paths.len());
    for p in paths {
        let src = &p.sourceInfo;
        let tgt = &p.targetInfo;
        let mut d = Display::default();
        let ids = format!(
            "source {}/{}, target {}/{}",
            luid(src.adapterId),
            src.id,
            luid(tgt.adapterId),
            tgt.id
        );

        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: header::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                src.adapterId,
                src.id,
            ),
            ..Default::default()
        };
        // SAFETY: header first, size set by `header`.
        match unsafe { device_info(&mut source, "source name") } {
            Ok(()) => d.gdi_name = from_wide(&source.viewGdiDeviceName),
            Err(e) => errors.push(format!("display ({ids}): {e}")),
        }
        let who = if d.gdi_name.is_empty() {
            format!("display ({ids})")
        } else {
            format!("display {} ({ids})", d.gdi_name)
        };

        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: header::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(
                DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                tgt.adapterId,
                tgt.id,
            ),
            ..Default::default()
        };
        // SAFETY: header first, size set by `header`.
        match unsafe { device_info(&mut target, "target name") } {
            Ok(()) => {
                d.monitor_name = from_wide(&target.monitorFriendlyDeviceName);
                d.monitor_path = from_wide(&target.monitorDevicePath);
                d.sudovda = path_matches_monitor(&d.monitor_path, monitor_ids);
            }
            Err(e) => errors.push(format!("{who}: {e}")),
        }

        let mut color = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: header::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(
                DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                tgt.adapterId,
                tgt.id,
            ),
            ..Default::default()
        };
        // SAFETY: header first, size set by `header`.
        match unsafe { device_info(&mut color, "advanced color info") } {
            Ok(()) => {
                // SAFETY: every bit pattern is a valid u32.
                let bits = unsafe { color.Anonymous.value };
                d.advanced_color = Some(AdvancedColor {
                    supported: bits & 0x1 != 0,
                    enabled: bits & 0x2 != 0,
                    wide_color_enforced: bits & 0x4 != 0,
                    bits_per_color_channel: color.bitsPerColorChannel,
                });
            }
            Err(e) => errors.push(format!("{who}: {e}")),
        }

        // Without QDC_VIRTUAL_MODE_AWARE the union holds a plain index.
        // SAFETY: every bit pattern is a valid u32.
        let idx = unsafe { src.Anonymous.modeInfoIdx } as usize;
        if let Some(m) = modes
            .get(idx)
            .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
        {
            // SAFETY: infoType says the union holds a source mode.
            let mode = unsafe { m.Anonymous.sourceMode };
            d.width = mode.width;
            d.height = mode.height;
            d.x = mode.position.x;
            d.y = mode.position.y;
            // DISPLAYCONFIG_SOURCE_MODE docs: the source at (0, 0) is
            // always the primary.
            d.primary = d.x == 0 && d.y == 0;
        }

        let rate = tgt.refreshRate;
        d.refresh_hz = (rate.Denominator != 0)
            .then(|| f64::from(rate.Numerator) / f64::from(rate.Denominator));
        list.push(d);
    }
    list
}

// ---- Setup around the driver -------------------------------------------

fn setup(errors: &mut Vec<String>) -> Setup {
    let mut setup = Setup::default();
    match crate::driver::read_health() {
        Ok(h) => setup.driver_health = Some(h),
        Err(e) => errors.push(format!("driver health: {e}")),
    }
    match steam_path() {
        Ok(path) => {
            let path = path.as_deref();
            // A read error must not show as "no flag file".
            match path.map(|p| p.join(checks::CEF_DEBUG_FLAG).try_exists()) {
                None => setup.steam_debug_flag = Some(false),
                Some(Ok(found)) => setup.steam_debug_flag = Some(found),
                Some(Err(e)) => errors.push(format!("Steam debug flag file: {e}")),
            }
            match steam(path) {
                Ok(s) => setup.steam = Some(s),
                Err(e) => errors.push(format!("Steam settings: {e}")),
            }
        }
        Err(e) => errors.push(format!("Steam settings: {e}")),
    }
    match autostart() {
        Ok(a) => setup.autostart = Some(a),
        Err(e) => errors.push(format!("logon start: {e}")),
    }
    match watcher_running() {
        Ok(r) => setup.watcher_running = Some(r),
        Err(e) => errors.push(format!("watcher lock: {e}")),
    }
    match crate::layout::session_path() {
        Ok(p) => setup.session_file = Some(p.is_file()),
        Err(e) => errors.push(format!("saved layout: {e}")),
    }
    setup
}

/// Steam's folder from `HKCU\Software\Valve\Steam\SteamPath`. `None`
/// when Steam was never installed for this Windows user.
pub(crate) fn steam_path() -> Result<Option<std::path::PathBuf>, String> {
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    let key = match windows_registry::CURRENT_USER.open(r"Software\Valve\Steam") {
        Ok(k) => k,
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Ok(None),
        Err(e) => return Err(format!("cannot open Steam's registry key: {e}")),
    };
    let path = key
        .get_string("SteamPath")
        .map_err(|e| format!("SteamPath not read: {e}"))?;
    Ok(Some(path.into()))
}

/// Reads Steam's Remote Play host options from the account's
/// `userdata\<account>\config\localconfig.vdf`. `steam_path` is `None`
/// when Steam is not installed.
fn steam(steam_path: Option<&std::path::Path>) -> Result<SteamSettings, String> {
    let none = || SteamSettings {
        account: checks::NO_ACCOUNT.into(),
        ..Default::default()
    };
    let Some(steam_path) = steam_path else {
        return Ok(none());
    };
    let userdata = steam_path.join("userdata");
    // Steam's running account. 0 when Steam is not running.
    let active = windows_registry::CURRENT_USER
        .open(r"Software\Valve\Steam\ActiveProcess")
        .and_then(|k| k.get_u32("ActiveUser"))
        .unwrap_or(0);
    let config = |id: &str| userdata.join(id).join("config").join("localconfig.vdf");
    let (file, account) = if active != 0 && config(&active.to_string()).is_file() {
        (config(&active.to_string()), "active")
    } else {
        // No userdata folder: no account signed in on this PC yet.
        let Ok(entries) = std::fs::read_dir(&userdata) else {
            return Ok(none());
        };
        let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
            .flatten()
            .map(|entry| entry.path().join("config").join("localconfig.vdf"))
            .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
            .collect();
        files.sort();
        let account = if files.len() == 1 { "only" } else { "newest" };
        let Some((_, file)) = files.pop() else {
            return Ok(none());
        };
        (file, account)
    };
    let bytes = std::fs::read(&file).map_err(|e| format!("cannot read localconfig.vdf: {e}"))?;
    Ok(checks::steam_settings(
        &String::from_utf8_lossy(&bytes),
        account,
    ))
}

/// The watcher's `Run` value and its switch in Settings > Apps > Startup.
fn autostart() -> Result<Autostart, String> {
    use crate::watcher::{RUN_KEY, RUN_VALUE};
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    let missing = ERROR_FILE_NOT_FOUND.to_hresult();
    let key = match windows_registry::CURRENT_USER.open(RUN_KEY) {
        Ok(k) => k,
        // No Run key at all: nothing starts at logon.
        Err(e) if e.code() == missing => return Ok(Autostart::default()),
        Err(e) => return Err(format!("cannot open the Run key: {e}")),
    };
    let command = match key.get_string(RUN_VALUE) {
        Ok(c) => c,
        Err(e) if e.code() == missing => return Ok(Autostart::default()),
        Err(e) => return Err(format!("cannot read the Run value: {e}")),
    };
    let program = checks::command_program(&command);
    // Explorer keeps the switch as a binary value: an odd first byte
    // means off. No value means on.
    let disabled = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
        .and_then(|k| k.get_value(RUN_VALUE))
        .is_ok_and(|v| v.first().is_some_and(|b| b & 1 == 1));
    let profile = std::env::var("USERPROFILE").ok();
    Ok(Autostart {
        program_exists: Some(std::path::Path::new(program).is_file()),
        command: Some(checks::hide_profile(&command, profile.as_deref())),
        disabled: Some(disabled),
    })
}

/// A watcher holds its single-instance lock while it runs.
pub fn watcher_running() -> Result<bool, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND};
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    // SAFETY: constant name; the handle is closed at once.
    match unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            crate::watcher::LOCK_NAME,
        )
    } {
        Ok(h) => {
            // SAFETY: handle from the call above.
            let _ = unsafe { CloseHandle(h) };
            Ok(true)
        }
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(false),
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}
