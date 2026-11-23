//! Thin wrappers around the CCD display configuration API (Windows only).

use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    SET_DISPLAY_CONFIG_FLAGS, SetDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID};

/// Text up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Request header for the request type `T`.
pub fn header<T>(
    kind: DISPLAYCONFIG_DEVICE_INFO_TYPE,
    adapter: LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: size_of::<T>() as u32,
        adapterId: adapter,
        id,
    }
}

/// Sends a `DisplayConfigGetDeviceInfo` request.
///
/// # Safety
/// `T` must be a `DISPLAYCONFIG_*` request type whose first field is the
/// header, and the header must name `T`'s size.
pub unsafe fn device_info<T>(packet: &mut T, what: &str) -> Result<(), String> {
    // SAFETY: per the contract, `packet` is a whole request of `size` bytes.
    let r = unsafe { DisplayConfigGetDeviceInfo((packet as *mut T).cast()) };
    if r == ERROR_SUCCESS.0 as i32 {
        Ok(())
    } else {
        Err(format!("{what} failed (error {r})"))
    }
}

/// Adapter LUID as `high:low` in hex.
pub fn luid(l: LUID) -> String {
    format!("{:08x}:{:08x}", l.HighPart as u32, l.LowPart)
}

/// Active paths and their modes (`QDC_ONLY_ACTIVE_PATHS`). Without
/// `QDC_VIRTUAL_MODE_AWARE`, the mode index unions hold plain indexes.
pub fn active_paths() -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>), String>
{
    // The topology can change between the size and the data query; then
    // QueryDisplayConfig returns ERROR_INSUFFICIENT_BUFFER.
    loop {
        let (mut np, mut nm) = (0u32, 0u32);
        // SAFETY: valid out pointers.
        let r = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) };
        if r != ERROR_SUCCESS {
            return Err(format!(
                "GetDisplayConfigBufferSizes failed (error {})",
                r.0
            ));
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        // SAFETY: the arrays hold `np` and `nm` elements.
        let r = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut np,
                paths.as_mut_ptr(),
                &mut nm,
                modes.as_mut_ptr(),
                None,
            )
        };
        if r == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if r != ERROR_SUCCESS {
            return Err(format!("QueryDisplayConfig failed (error {})", r.0));
        }
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        return Ok((paths, modes));
    }
}

/// Calls `SetDisplayConfig`. Empty arrays are passed as NULL, as the
/// topology flags (for example `SDC_USE_DATABASE_CURRENT`) require.
pub fn set_config(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
    flags: SET_DISPLAY_CONFIG_FLAGS,
) -> Result<(), String> {
    // SAFETY: the slices are live for the call.
    let r = unsafe {
        SetDisplayConfig(
            (!paths.is_empty()).then_some(paths),
            (!modes.is_empty()).then_some(modes),
            flags,
        )
    };
    if r == ERROR_SUCCESS.0 as i32 {
        Ok(())
    } else {
        Err(format!(
            "SetDisplayConfig (flags 0x{:x}) failed (error {r})",
            flags.0
        ))
    }
}

/// Turns advanced color (HDR) on or off for one target.
pub fn set_advanced_color(adapter: LUID, target: u32, enable: bool) -> Result<(), String> {
    let mut packet = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
        header: header::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>(
            DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
            adapter,
            target,
        ),
        ..Default::default()
    };
    // Bit 0 is `enableAdvancedColor`; the other bits are reserved.
    packet.Anonymous.value = u32::from(enable);
    // SAFETY: header first, size set by `header`.
    let r = unsafe { DisplayConfigSetDeviceInfo(&packet.header) };
    if r == ERROR_SUCCESS.0 as i32 {
        Ok(())
    } else {
        Err(format!("set advanced color failed (error {r})"))
    }
}
