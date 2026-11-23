//! Control protocol of the SudoVDA virtual display driver.
//!
//! The values come from `sudovda-ioctl.h`. The header is identical in
//! SudoMaker/SudoVDA (`Common/Include/sudovda-ioctl.h`, commit a4b09fa) and
//! ClassicOldSong/Apollo (`third-party/sudovda/sudovda-ioctl.h`, commit
//! adc5c5a). The driver side is `SudoVDAIoDeviceControl` in `Driver.cpp`.
//!
//! The protocol types are plain data and build on every platform, so the
//! layout tests also run on Linux. [`Device`] (Windows only) opens the driver
//! and sends the requests.
//!
//! Watchdog side effect: every request except [`IOCTL_GET_WATCHDOG`] resets
//! the driver's watchdog countdown, for all virtual displays at once
//! (`Driver.cpp`, top of `SudoVDAIoDeviceControl`). A ping or a version query
//! from this crate therefore keeps Steam's displays alive a little longer.

#![warn(missing_docs)]

#[cfg(windows)]
mod device;

#[cfg(windows)]
pub use device::{Device, Error, interface_paths, read_cm_list};

/// Encodes a string as NUL-terminated UTF-16 for a `PCWSTR` argument.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Splits a NUL-separated, double-NUL-terminated UTF-16 list (as returned
/// by `CM_Get_Device_Interface_ListW`) into strings.
pub fn split_multi_sz(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Hardware ID of the SudoVDA root-enumerated device node.
pub const HARDWARE_ID: &str = r"root\sudomaker\sudovda";

/// Display device setup class `{4d36e968-e325-11ce-bfc1-08002be10318}`.
pub const CLASS_GUID: Guid = Guid {
    data1: 0x4d36_e968,
    data2: 0xe325,
    data3: 0x11ce,
    data4: [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
};

/// Device interface of the control device `{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}`.
pub const INTERFACE_GUID: Guid = Guid {
    data1: 0xe5bc_c234,
    data2: 0x1e0c,
    data3: 0x418a,
    data4: [0xa0, 0xd4, 0xef, 0x8b, 0x75, 0x01, 0x41, 0x4d],
};

/// Registry key the driver reads at load time (`LoadSettings` in `Driver.cpp`).
/// Values: `gpuName`, `testMode`, `watchdog`, `maxMonitors`, `sdrBits`, `hdrBits`.
pub const REGISTRY_KEY: &str = r"SOFTWARE\SudoMaker\SudoVDA";

/// Protocol version in the public source (`VDAProtocolVersion`). The shipped
/// driver can report another value; read it with [`Device::protocol_version`].
pub const SOURCE_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: 0,
    minor: 2,
    incremental: 1,
    test_build: 1,
};

const FILE_DEVICE_UNKNOWN: u32 = 0x22;
const METHOD_BUFFERED: u32 = 0;
const FILE_ANY_ACCESS: u32 = 0;

/// The `CTL_CODE` macro from the Windows DDK.
pub const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const fn sudovda_ioctl(function: u32) -> u32 {
    ctl_code(
        FILE_DEVICE_UNKNOWN,
        function,
        METHOD_BUFFERED,
        FILE_ANY_ACCESS,
    )
}

/// Add a virtual display. In: [`AddParams`]. Out: [`AddOut`].
pub const IOCTL_ADD_VIRTUAL_DISPLAY: u32 = sudovda_ioctl(0x800);
/// Remove a virtual display. In: [`RemoveParams`].
pub const IOCTL_REMOVE_VIRTUAL_DISPLAY: u32 = sudovda_ioctl(0x801);
/// Select the render GPU. In: [`SetRenderAdapterParams`].
pub const IOCTL_SET_RENDER_ADAPTER: u32 = sudovda_ioctl(0x802);
/// Read the watchdog state. Out: [`WatchdogOut`]. Does not reset the watchdog.
pub const IOCTL_GET_WATCHDOG: u32 = sudovda_ioctl(0x803);
/// Keep-alive. No buffers.
pub const IOCTL_DRIVER_PING: u32 = sudovda_ioctl(0x888);
/// Read the protocol version. Out: [`ProtocolVersion`].
pub const IOCTL_GET_PROTOCOL_VERSION: u32 = sudovda_ioctl(0x8FF);

/// Windows `GUID`, same layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Guid {
    /// First 32 bits.
    pub data1: u32,
    /// Next 16 bits.
    pub data2: u16,
    /// Next 16 bits.
    pub data3: u16,
    /// Last 64 bits.
    pub data4: [u8; 8],
}

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = &self.data4;
        write!(
            f,
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

/// Windows `LUID`, same layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Luid {
    /// Low 32 bits.
    pub low_part: u32,
    /// High 32 bits.
    pub high_part: i32,
}

/// `VIRTUAL_DISPLAY_ADD_PARAMS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddParams {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Refresh rate in Hz. The driver multiplies values below 1000 by 1000.
    pub refresh_rate: u32,
    /// Identity of the monitor. Adding a known GUID returns the existing one.
    pub monitor_guid: Guid,
    /// Name for the EDID, not NUL-terminated if all 14 bytes are used.
    pub device_name: [u8; 14],
    /// Serial number for the EDID.
    pub serial_number: [u8; 14],
}

/// `VIRTUAL_DISPLAY_REMOVE_PARAMS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveParams {
    /// Identity given at add time.
    pub monitor_guid: Guid,
}

/// `VIRTUAL_DISPLAY_ADD_OUT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddOut {
    /// Adapter of the new monitor.
    pub adapter_luid: Luid,
    /// CCD target ID of the new monitor.
    pub target_id: u32,
}

/// `VIRTUAL_DISPLAY_SET_RENDER_ADAPTER_PARAMS`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetRenderAdapterParams {
    /// GPU that renders the virtual displays.
    pub adapter_luid: Luid,
}

/// `VIRTUAL_DISPLAY_GET_WATCHDOG_OUT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchdogOut {
    /// Timeout in seconds. 0 means off.
    pub timeout: u32,
    /// Seconds left before all virtual displays are removed.
    pub countdown: u32,
}

/// `SUVDA_PROTOCAL_VERSION` (spelling as in the header).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolVersion {
    /// Major version.
    pub major: u8,
    /// Minor version.
    pub minor: u8,
    /// Incremental version.
    pub incremental: u8,
    /// C++ `bool` in the header. Kept as `u8` because the driver writes it:
    /// a value other than 0 or 1 in a Rust `bool` is undefined behaviour.
    pub test_build: u8,
}

impl ProtocolVersion {
    /// True for a test build.
    pub fn is_test_build(&self) -> bool {
        self.test_build != 0
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.incremental)?;
        if self.is_test_build() {
            f.write_str(" (test build)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn ioctl_codes_match_driver() {
        assert_eq!(IOCTL_ADD_VIRTUAL_DISPLAY, 0x0022_2000);
        assert_eq!(IOCTL_REMOVE_VIRTUAL_DISPLAY, 0x0022_2004);
        assert_eq!(IOCTL_SET_RENDER_ADAPTER, 0x0022_2008);
        assert_eq!(IOCTL_GET_WATCHDOG, 0x0022_200C);
        assert_eq!(IOCTL_DRIVER_PING, 0x0022_2220);
        assert_eq!(IOCTL_GET_PROTOCOL_VERSION, 0x0022_23FC);
    }

    #[test]
    fn struct_layout_matches_msvc() {
        // MSVC x64: GUID is 16 bytes with 4-byte alignment, LUID is 8 bytes.
        assert_eq!((size_of::<Guid>(), align_of::<Guid>()), (16, 4));
        assert_eq!((size_of::<Luid>(), align_of::<Luid>()), (8, 4));

        assert_eq!(size_of::<AddParams>(), 56);
        assert_eq!(offset_of!(AddParams, refresh_rate), 8);
        assert_eq!(offset_of!(AddParams, monitor_guid), 12);
        assert_eq!(offset_of!(AddParams, device_name), 28);
        assert_eq!(offset_of!(AddParams, serial_number), 42);

        assert_eq!(size_of::<RemoveParams>(), 16);
        assert_eq!(size_of::<AddOut>(), 12);
        assert_eq!(offset_of!(AddOut, target_id), 8);
        assert_eq!(size_of::<SetRenderAdapterParams>(), 8);
        assert_eq!(size_of::<WatchdogOut>(), 8);
        assert_eq!(size_of::<ProtocolVersion>(), 4);
    }

    #[test]
    fn guid_display() {
        assert_eq!(
            INTERFACE_GUID.to_string(),
            "{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}"
        );
        assert_eq!(
            CLASS_GUID.to_string(),
            "{4d36e968-e325-11ce-bfc1-08002be10318}"
        );
    }

    #[test]
    fn split_multi_sz_handles_empty_and_lists() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert!(split_multi_sz(&[]).is_empty());
        assert!(split_multi_sz(&[0]).is_empty());
        let mut buf = w(r"\\?\a");
        buf.push(0);
        buf.extend(w(r"\\?\b"));
        buf.extend([0, 0]);
        assert_eq!(split_multi_sz(&buf), [r"\\?\a", r"\\?\b"]);
    }

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide(""), [0]);
        assert_eq!(wide("ab"), [u16::from(b'a'), u16::from(b'b'), 0]);
    }

    #[test]
    fn protocol_version_display() {
        assert_eq!(SOURCE_PROTOCOL_VERSION.to_string(), "0.2.1 (test build)");
    }
}
