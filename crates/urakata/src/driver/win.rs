//! Windows part of the driver commands: reads the state and applies the
//! steps of a plan. The changes need an elevated process.

use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVNODE_STATUS_FLAGS, CM_Get_DevNode_Status, CM_PROB, CR_SUCCESS, DICD_GENERATE_ID,
    DIF_REGISTERDEVICE, DIINSTALLDRIVER_FLAGS, DIIRFLAG_FORCE_INF, DN_HAS_PROBLEM, DN_STARTED,
    DiInstallDriverW, DiUninstallDevice, GUID_DEVCLASS_DISPLAY, HDEVINFO,
    SETUP_DI_GET_CLASS_DEVS_FLAGS, SP_DEVINFO_DATA, SPDRP_HARDWAREID, SUOI_FORCEDELETE,
    SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDevicePropertyW, SetupDiGetDeviceRegistryPropertyW,
    SetupDiOpenDeviceInfoW, SetupDiSetDeviceRegistryPropertyW, SetupUninstallOEMInfW,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_DriverInfPath, DEVPKEY_Device_DriverVersion, DEVPROPTYPE,
};
use windows::Win32::Foundation::{
    CloseHandle, DEVPROPKEY, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS_REBOOT_REQUIRED,
    ERROR_SUCCESS_RESTART_REQUIRED, HANDLE, HWND,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_SHA256_ALG_HANDLE, BCryptHash, CERT_CONTEXT, CERT_FIND_SHA1_HASH, CERT_OPEN_STORE_FLAGS,
    CERT_SHA1_HASH_PROP_ID, CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_OPEN_EXISTING_FLAG,
    CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_LOCAL_MACHINE,
    CRYPT_INTEGER_BLOB, CertAddEncodedCertificateToStore, CertCloseStore,
    CertCreateCertificateContext, CertDeleteCertificateFromStore, CertFindCertificateInStore,
    CertFreeCertificateContext, CertGetCertificateContextProperty, CertOpenStore, HCERTSTORE,
    PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use super::{
    CERT_FILE, CERT_STORES, CERT_THUMBPRINT, DeviceState, Health, INF_FILE, PACKAGE_FILES, State,
    Step, hex, inf_matches_sudovda,
};

/// `FILE_SHARE_READ`: others may read, but not write or delete.
const FILE_SHARE_READ: u32 = 1;

/// Reads all SudoVDA device nodes, the certificate stores and whether a
/// stream runs. Needs no admin.
pub fn read_state() -> Result<State, String> {
    let stream_active = !crate::status::sudovda_monitor_ids()?.is_empty();
    Ok(State {
        devices: devices()?,
        certificate: certificate()?,
        stream_active,
        packages: packages()?,
    })
}

/// The health that `urakata driver status` shows, without the slower
/// parts of [`read_state`] that the health does not use (driver store scan,
/// monitor list). Needs no admin.
pub fn read_health() -> Result<Health, String> {
    let state = State {
        devices: devices()?,
        certificate: certificate()?,
        ..State::default()
    };
    Ok(state.health())
}

/// The certificate is in each of [`CERT_STORES`], same order.
fn certificate() -> Result<[bool; 2], String> {
    let thumbprint = thumbprint_bytes();
    let mut certificate = [false; 2];
    for (slot, store) in certificate.iter_mut().zip(CERT_STORES) {
        let store = Store::open(store, false)?;
        *slot = store.find(&thumbprint).map(|c| c.free()).is_some();
    }
    Ok(certificate)
}

/// Published `oem*.inf` files in `%WINDIR%\INF` for the SudoVDA hardware
/// ID. A file that cannot be read is skipped.
fn packages() -> Result<Vec<String>, String> {
    let windir = std::env::var_os("WINDIR").ok_or("WINDIR is not set")?;
    let dir = Path::new(&windir).join("INF");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if !(lower.starts_with("oem") && lower.ends_with(".inf")) {
            continue;
        }
        if std::fs::read(entry.path()).is_ok_and(|b| inf_matches_sudovda(&b)) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// A checked package. The package files stay open without write or
/// delete sharing until this is dropped, so they cannot change between the
/// hash check and the install.
pub struct VerifiedPackage {
    /// Certificate bytes, checked against the pinned thumbprint.
    pub cert: Vec<u8>,
    _locks: Vec<File>,
}

/// Opens `path` for reading with read sharing only and reads it.
fn read_locked(path: &Path) -> Result<(File, Vec<u8>), String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok((file, data))
}

/// Checks the package folder against the pinned hashes and thumbprint and
/// keeps the files locked (see [`VerifiedPackage`]).
pub fn verify_package(dir: &Path) -> Result<VerifiedPackage, String> {
    let mut locks = Vec::new();
    for (name, expected) in PACKAGE_FILES {
        let path = dir.join(name);
        let (file, data) = read_locked(&path)?;
        locks.push(file);
        let got = sha256(&data)?;
        if !got.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "{} has SHA-256 {got}, expected {expected}; the package is changed or damaged",
                path.display()
            ));
        }
    }
    let path = dir.join(CERT_FILE);
    let (file, cert) = read_locked(&path)?;
    locks.push(file);
    let got = cert_thumbprint(&cert)?;
    if got != CERT_THUMBPRINT {
        return Err(format!(
            "{} has thumbprint {got}, expected {CERT_THUMBPRINT}",
            path.display()
        ));
    }
    Ok(VerifiedPackage {
        cert,
        _locks: locks,
    })
}

/// Result of [`apply`].
pub struct Applied {
    /// One line per step.
    pub log: Vec<String>,
    /// Windows asked for a restart to finish.
    pub reboot: bool,
}

/// Runs the steps in order and stops at the first failure. `package` must
/// have passed [`verify_package`] when a step installs from it.
pub fn apply(steps: &[Step], package: &Path, cert: &[u8]) -> Result<Applied, (Applied, String)> {
    let mut done = Applied {
        log: Vec::new(),
        reboot: false,
    };
    if !steps.is_empty() && !is_elevated() {
        return Err((
            done,
            "this needs admin rights; run it from an elevated prompt".into(),
        ));
    }
    for step in steps {
        let result = match step {
            Step::AddCertificate { store } => Store::open(store, true)
                .and_then(|s| s.add(cert))
                .map(|()| false),
            Step::RemoveCertificate { store } => Store::open(store, true)
                .and_then(|s| s.remove(&thumbprint_bytes()))
                .map(|()| false),
            Step::CreateDevice => create_device().map(|()| false),
            Step::InstallDriver { force } => install_driver(&package.join(INF_FILE), *force),
            Step::RemoveDevice { instance_id } => remove_device(instance_id),
            Step::DeletePackage { inf } => delete_package(inf).map(|()| false),
        };
        match result {
            Ok(reboot) => {
                done.reboot |= reboot;
                done.log.push(format!("done: {step}"));
            }
            Err(e) => return Err((done, format!("{step}: {e}"))),
        }
    }
    Ok(done)
}

/// The process token is elevated.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle needs no close; `token` is a valid out
    // pointer and is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the buffer is a TOKEN_ELEVATION of the given size.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .is_ok();
    // SAFETY: `token` was opened above.
    let _ = unsafe { CloseHandle(token) };
    ok && elevation.TokenIsElevated != 0
}

// ---- Hashes ------------------------------------------------------------

/// Upper-case hex SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> Result<String, String> {
    let mut out = [0u8; 32];
    // SAFETY: the pseudo handle needs no open; slices are valid.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.is_ok() {
        Ok(hex(&out))
    } else {
        Err(format!("BCryptHash failed (NTSTATUS 0x{:08X})", status.0))
    }
}

fn thumbprint_bytes() -> [u8; 20] {
    let mut out = [0u8; 20];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&CERT_THUMBPRINT[2 * i..2 * i + 2], 16).unwrap_or(0);
    }
    out
}

fn cert_thumbprint(der: &[u8]) -> Result<String, String> {
    // SAFETY: `der` is a valid slice; the context is freed below.
    let ctx = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, der) };
    if ctx.is_null() {
        return Err(format!(
            "{CERT_FILE} is not a DER certificate: {}",
            windows::core::Error::from_thread()
        ));
    }
    let ctx = CertRef(ctx);
    let mut out = [0u8; 20];
    let mut len = out.len() as u32;
    // SAFETY: `out` holds `len` bytes.
    unsafe {
        CertGetCertificateContextProperty(
            ctx.0,
            CERT_SHA1_HASH_PROP_ID,
            Some(out.as_mut_ptr().cast()),
            &mut len,
        )
    }
    .map_err(|e| format!("certificate thumbprint: {e}"))?;
    ctx.free();
    Ok(hex(&out[..len as usize]))
}

// ---- Certificate stores ------------------------------------------------

/// A certificate context. Freed by [`CertRef::free`] or on drop.
struct CertRef(*mut CERT_CONTEXT);

impl CertRef {
    fn free(self) {}
}

impl Drop for CertRef {
    fn drop(&mut self) {
        // SAFETY: the context came from CertCreate or CertFind and is
        // freed once.
        unsafe {
            let _ = CertFreeCertificateContext(Some(self.0));
        }
    }
}

struct Store(HCERTSTORE);

impl Store {
    fn open(name: &str, write: bool) -> Result<Self, String> {
        let wide = sudovda::wide(name);
        let mut flags = CERT_SYSTEM_STORE_LOCAL_MACHINE | CERT_STORE_OPEN_EXISTING_FLAG.0;
        if !write {
            flags |= CERT_STORE_READONLY_FLAG.0;
        }
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                Default::default(),
                None,
                CERT_OPEN_STORE_FLAGS(flags),
                Some(wide.as_ptr().cast()),
            )
        }
        .map(Self)
        .map_err(|e| format!("cannot open LocalMachine\\{name}: {e}"))
    }

    fn find(&self, thumbprint: &[u8; 20]) -> Option<CertRef> {
        let blob = CRYPT_INTEGER_BLOB {
            cbData: thumbprint.len() as u32,
            pbData: thumbprint.as_ptr().cast_mut(),
        };
        // SAFETY: `blob` points to `thumbprint`, which outlives the call.
        let ctx = unsafe {
            CertFindCertificateInStore(
                self.0,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_SHA1_HASH,
                Some((&blob as *const CRYPT_INTEGER_BLOB).cast()),
                None,
            )
        };
        (!ctx.is_null()).then_some(CertRef(ctx))
    }

    fn add(&self, der: &[u8]) -> Result<(), String> {
        // SAFETY: `der` is a valid slice.
        unsafe {
            CertAddEncodedCertificateToStore(
                Some(self.0),
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                der,
                CERT_STORE_ADD_REPLACE_EXISTING,
                None,
            )
        }
        .map_err(|e| e.to_string())
    }

    fn remove(&self, thumbprint: &[u8; 20]) -> Result<(), String> {
        let Some(ctx) = self.find(thumbprint) else {
            return Ok(());
        };
        // CertDeleteCertificateFromStore frees the context, also on error.
        let raw = ctx.0;
        std::mem::forget(ctx);
        // SAFETY: `raw` came from CertFindCertificateInStore.
        unsafe { CertDeleteCertificateFromStore(raw) }.map_err(|e| e.to_string())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: the store was opened by CertOpenStore.
        unsafe {
            let _ = CertCloseStore(Some(self.0), 0);
        }
    }
}

// ---- Device nodes ------------------------------------------------------

struct DevInfo(HDEVINFO);

impl Drop for DevInfo {
    fn drop(&mut self) {
        // SAFETY: the set was created by SetupDi and is destroyed once.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn devinfo_data() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    }
}

/// All Display-class nodes with the SudoVDA hardware ID, present or not.
fn devices() -> Result<Vec<DeviceState>, String> {
    // Flags 0: all devices of the class, also the ones not present.
    // SAFETY: valid class GUID pointer.
    let set = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_DISPLAY),
            PCWSTR::null(),
            None,
            SETUP_DI_GET_CLASS_DEVS_FLAGS(0),
        )
    }
    .map_err(|e| format!("SetupDiGetClassDevs: {e}"))?;
    let set = DevInfo(set);
    let mut out = Vec::new();
    for index in 0.. {
        let mut data = devinfo_data();
        // SAFETY: `data.cbSize` is set.
        if let Err(e) = unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) } {
            if e.code() == ERROR_NO_MORE_ITEMS.to_hresult() {
                break;
            }
            return Err(format!("SetupDiEnumDeviceInfo: {e}"));
        }
        let ids = hardware_ids(&set, &data)?;
        if !ids
            .iter()
            .any(|h| h.eq_ignore_ascii_case(sudovda::HARDWARE_ID))
        {
            continue;
        }
        out.push(device_state(&set, &data)?);
    }
    Ok(out)
}

fn hardware_ids(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Result<Vec<String>, String> {
    let mut buf = vec![0u8; 1024];
    loop {
        let mut needed = 0u32;
        // SAFETY: `buf` is a live, writable buffer.
        let r = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set.0,
                data,
                SPDRP_HARDWAREID,
                None,
                Some(&mut buf),
                Some(&mut needed),
            )
        };
        match r {
            Ok(()) => {
                let wide: Vec<u16> = buf[..needed as usize]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                return Ok(sudovda::split_multi_sz(&wide));
            }
            Err(_) if needed as usize > buf.len() => buf.resize(needed as usize, 0),
            // A node without hardware IDs (for example one being created).
            Err(_) => return Ok(Vec::new()),
        }
    }
}

fn device_state(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Result<DeviceState, String> {
    let mut id = [0u16; 400];
    let mut len = 0u32;
    // SAFETY: `id` is a live, writable buffer.
    unsafe { SetupDiGetDeviceInstanceIdW(set.0, data, Some(&mut id), Some(&mut len)) }
        .map_err(|e| format!("SetupDiGetDeviceInstanceId: {e}"))?;
    let instance_id = String::from_utf16_lossy(&id[..len.saturating_sub(1) as usize]);
    let mut status = CM_DEVNODE_STATUS_FLAGS::default();
    let mut problem = CM_PROB::default();
    // SAFETY: valid out pointers. A node that is not present returns
    // CR_NO_SUCH_DEVINST.
    let cr = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, data.DevInst, 0) };
    let present = cr == CR_SUCCESS;
    Ok(DeviceState {
        instance_id,
        present,
        started: present && status.0 & DN_STARTED.0 != 0,
        problem_code: (present && status.0 & DN_HAS_PROBLEM.0 != 0).then_some(problem.0),
        driver_version: string_property(set, data, &DEVPKEY_Device_DriverVersion),
        driver_inf: string_property(set, data, &DEVPKEY_Device_DriverInfPath),
    })
}

/// A string property, or `None` when it is not set.
fn string_property(set: &DevInfo, data: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let mut buf = vec![0u8; 512];
    let mut ty = DEVPROPTYPE::default();
    let mut needed = 0u32;
    // SAFETY: `buf` is a live, writable buffer.
    unsafe {
        SetupDiGetDevicePropertyW(
            set.0,
            data,
            key,
            &mut ty,
            Some(&mut buf),
            Some(&mut needed),
            0,
        )
    }
    .ok()?;
    let wide: Vec<u16> = buf[..needed as usize]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16_lossy(&wide);
    let s = s.trim_end_matches('\0');
    (!s.is_empty()).then(|| s.to_string())
}

/// Creates a root node of the Display class with the SudoVDA hardware ID
/// (as devcon `install` and nefconc `--create-device-node`).
fn create_device() -> Result<(), String> {
    // SAFETY: valid class GUID pointer.
    let set = unsafe { SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_DISPLAY), None) }
        .map_err(|e| format!("SetupDiCreateDeviceInfoList: {e}"))?;
    let set = DevInfo(set);
    let mut data = devinfo_data();
    let class_name = sudovda::wide("Display");
    // SAFETY: NUL-terminated name, valid GUID and out pointers.
    unsafe {
        SetupDiCreateDeviceInfoW(
            set.0,
            PCWSTR(class_name.as_ptr()),
            &GUID_DEVCLASS_DISPLAY,
            PCWSTR::null(),
            None,
            DICD_GENERATE_ID,
            Some(&mut data),
        )
    }
    .map_err(|e| format!("SetupDiCreateDeviceInfo: {e}"))?;
    // REG_MULTI_SZ: the ID, then an empty string.
    let mut ids = sudovda::wide(sudovda::HARDWARE_ID);
    ids.push(0);
    let bytes: Vec<u8> = ids.iter().flat_map(|w| w.to_le_bytes()).collect();
    // SAFETY: `bytes` is a valid slice.
    unsafe { SetupDiSetDeviceRegistryPropertyW(set.0, &mut data, SPDRP_HARDWAREID, Some(&bytes)) }
        .map_err(|e| format!("set hardware ID: {e}"))?;
    // SAFETY: `data` belongs to `set`.
    unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, Some(&data)) }
        .map_err(|e| format!("register the device: {e}"))
}

/// Windows codes that mean "done, restart to finish" (a test
/// install got 0xBC3 and still started the device).
fn is_reboot_code(e: &windows::core::Error) -> bool {
    e.code() == ERROR_SUCCESS_REBOOT_REQUIRED.to_hresult()
        || e.code() == ERROR_SUCCESS_RESTART_REQUIRED.to_hresult()
}

/// Installs the INF on matching devices. Returns whether Windows asks for
/// a restart.
fn install_driver(inf: &Path, force: bool) -> Result<bool, String> {
    let full = std::path::absolute(inf).map_err(|e| format!("{}: {e}", inf.display()))?;
    let wide: Vec<u16> = full.as_os_str().encode_wide().chain([0]).collect();
    let flags = if force {
        DIIRFLAG_FORCE_INF
    } else {
        DIINSTALLDRIVER_FLAGS(0)
    };
    let mut reboot = windows::core::BOOL::default();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    match unsafe { DiInstallDriverW(None, PCWSTR(wide.as_ptr()), flags, Some(&mut reboot)) } {
        Ok(()) => Ok(reboot.as_bool()),
        Err(e) if is_reboot_code(&e) => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}

fn remove_device(instance_id: &str) -> Result<bool, String> {
    // SAFETY: valid class GUID pointer.
    let set = unsafe { SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_DISPLAY), None) }
        .map_err(|e| format!("SetupDiCreateDeviceInfoList: {e}"))?;
    let set = DevInfo(set);
    let mut data = devinfo_data();
    let wide = sudovda::wide(instance_id);
    // SAFETY: NUL-terminated ID, valid out pointer.
    unsafe { SetupDiOpenDeviceInfoW(set.0, PCWSTR(wide.as_ptr()), None, 0, Some(&mut data)) }
        .map_err(|e| format!("SetupDiOpenDeviceInfo: {e}"))?;
    let mut reboot = windows::core::BOOL::default();
    // SAFETY: `data` belongs to `set`.
    match unsafe { DiUninstallDevice(HWND::default(), set.0, &data, 0, Some(&mut reboot)) } {
        Ok(()) => Ok(reboot.as_bool()),
        Err(e) if is_reboot_code(&e) => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}

fn delete_package(inf: &str) -> Result<(), String> {
    let wide = sudovda::wide(inf);
    // SAFETY: NUL-terminated name.
    let ok = unsafe { SetupUninstallOEMInfW(PCWSTR(wide.as_ptr()), SUOI_FORCEDELETE, None) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(windows::core::Error::from_thread().to_string())
    }
}
