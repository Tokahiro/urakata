//! Windows part of the update: HTTPS through WinHTTP (system proxy and
//! certificate store), download and installer start.

use std::path::PathBuf;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_URL, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpQueryOption,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
};
use windows::core::PCWSTR;

use super::{
    CURRENT, LATEST_URL, Release, final_host_allowed, newer_release, parse_sha256_file, split_url,
};

/// Largest download accepted (the installer is about 10 MB).
const MAX_BYTES: usize = 200 * 1024 * 1024;

/// Asks GitHub for the latest release. `Ok(None)`: up to date.
pub fn check() -> Result<Option<Release>, String> {
    let json = get(
        LATEST_URL,
        "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28",
    )
    .map_err(|e| {
        if e.contains("HTTP 404") {
            "no published release found (the repository may be private)".to_string()
        } else {
            e
        }
    })?;
    newer_release(&json, CURRENT)
}

/// Downloads the installer, checks its hash and starts it silently. The
/// installer stops the watcher and the settings window, replaces the
/// files and starts the watcher again, so this process must end soon.
/// Refuses while a stream runs, because the watcher would stop during it.
pub fn install(release: &Release) -> Result<String, String> {
    // An elevated installer would start the watcher elevated, and a later
    // normal update could not stop it.
    if crate::driver::is_elevated() {
        return Err("run `urakata update` without admin rights".into());
    }
    if !crate::status::sudovda_monitor_ids()?.is_empty() {
        return Err("a SudoVDA display is active (a stream runs); update after the stream".into());
    }
    let sums = get(&release.sha256_url, "")?;
    let expected = parse_sha256_file(&String::from_utf8_lossy(&sums), &release.setup_name)
        .ok_or("the .sha256 file of the release is not valid")?;
    let setup = get(&release.setup_url, "")?;
    let got = crate::driver::sha256(&setup)?;
    if got != expected {
        return Err(format!(
            "{} has SHA-256 {got}, expected {expected}; not started",
            release.setup_name
        ));
    }
    let dir: PathBuf = std::env::temp_dir().join("urakata-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(&release.setup_name);
    std::fs::write(&path, &setup).map_err(|e| format!("{}: {e}", path.display()))?;
    std::process::Command::new(&path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", path.display()))?;
    Ok(format!(
        "Started the installer for urakata {} ({}). It runs without a window.",
        release.version,
        path.display()
    ))
}

/// The URL after redirects.
fn final_url(request: &Handle) -> Result<String, String> {
    let mut buf = vec![0u16; 4096];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `len` bytes.
    unsafe {
        WinHttpQueryOption(
            request.0,
            WINHTTP_OPTION_URL,
            Some(buf.as_mut_ptr().cast()),
            &mut len,
        )
    }
    .map_err(|e| format!("final URL: {e}"))?;
    let chars = (len as usize / 2).min(buf.len());
    Ok(String::from_utf16_lossy(&buf[..chars])
        .trim_end_matches('\0')
        .to_string())
}

/// Closes a WinHTTP handle on drop.
struct Handle(*mut core::ffi::c_void);

impl Handle {
    fn new(raw: *mut core::ffi::c_void, what: &str) -> Result<Self, String> {
        if raw.is_null() {
            Err(format!("{what}: {}", windows::core::Error::from_thread()))
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// GET over HTTPS. `headers`: extra request headers, CRLF separated.
/// Redirects to other HTTPS hosts are followed (GitHub asset downloads).
fn get(url: &str, headers: &str) -> Result<Vec<u8>, String> {
    let (host, path) = split_url(url)?;
    let agent = sudovda::wide(&format!("urakata/{CURRENT}"));
    // SAFETY: NUL-terminated agent; null proxy strings with automatic proxy.
    let session = Handle::new(
        unsafe {
            WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        },
        "WinHttpOpen",
    )?;
    let host_w = sudovda::wide(host);
    // SAFETY: valid session handle, NUL-terminated host.
    let connect = Handle::new(
        unsafe {
            WinHttpConnect(
                session.0,
                PCWSTR(host_w.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        },
        "WinHttpConnect",
    )?;
    let verb = sudovda::wide("GET");
    let path_w = sudovda::wide(path);
    // SAFETY: valid connect handle, NUL-terminated strings, null accept
    // types.
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connect.0,
                PCWSTR(verb.as_ptr()),
                PCWSTR(path_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "WinHttpOpenRequest",
    )?;
    let headers_w: Vec<u16> = headers.encode_utf16().collect();
    let headers_arg = (!headers_w.is_empty()).then_some(headers_w.as_slice());
    // SAFETY: valid request handle; the headers slice outlives the call.
    unsafe { WinHttpSendRequest(request.0, headers_arg, None, 0, 0, 0) }
        .map_err(|e| format!("{url}: {e}"))?;
    // SAFETY: valid request handle.
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
        .map_err(|e| format!("{url}: {e}"))?;
    let mut status = 0u32;
    let mut len = size_of::<u32>() as u32;
    // SAFETY: `status` holds `len` bytes.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| format!("{url}: status: {e}"))?;
    let final_url = final_url(&request)?;
    if !final_host_allowed(&final_url) {
        return Err(format!("{url}: redirected to a host that is not allowed"));
    }
    if status != 200 {
        return Err(format!("{url}: HTTP {status}"));
    }
    let mut body = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        // SAFETY: `chunk` holds `chunk.len()` bytes.
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        }
        .map_err(|e| format!("{url}: read: {e}"))?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
        if body.len() > MAX_BYTES {
            return Err(format!("{url}: larger than {MAX_BYTES} bytes"));
        }
    }
    Ok(body)
}
