//! Handle to the SudoVDA control device (Windows only).
//!
//! urakata itself only reads. Steam creates and removes its own virtual
//! displays. `Device::add_display` and `Device::remove_display` exist only
//! with the `host-test` feature, for host tests that need a virtual display
//! without a stream.

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW,
    CM_Get_Device_Interface_ListW, CONFIGRET, CR_BUFFER_SMALL, CR_SUCCESS,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{GUID, PCWSTR};

use crate::{
    INTERFACE_GUID, IOCTL_DRIVER_PING, IOCTL_GET_PROTOCOL_VERSION, IOCTL_GET_WATCHDOG,
    ProtocolVersion, WatchdogOut, split_multi_sz, wide,
};

/// Errors from opening or talking to the driver.
#[derive(Debug)]
pub enum Error {
    /// No present device exposes the SudoVDA interface.
    NotFound,
    /// The configuration manager returned this `CONFIGRET`.
    ConfigManager(u32),
    /// A Win32 call failed.
    Windows(windows::core::Error),
    /// The driver returned fewer bytes than the output structure needs.
    ShortReply {
        /// Bytes expected.
        expected: usize,
        /// Bytes returned.
        got: u32,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("SudoVDA control device not found"),
            Self::ConfigManager(cr) => write!(f, "configuration manager error 0x{cr:x}"),
            Self::Windows(e) => write!(f, "{e}"),
            Self::ShortReply { expected, got } => {
                write!(f, "driver returned {got} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows(e) => Some(e),
            _ => None,
        }
    }
}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Self::Windows(e)
    }
}

/// Open handle to the SudoVDA control device. Closed on drop.
#[derive(Debug)]
pub struct Device {
    handle: HANDLE,
    path: String,
}

impl Device {
    /// Opens the first present device with the SudoVDA interface, with read
    /// and write access.
    pub fn open() -> Result<Self, Error> {
        Self::open_with((GENERIC_READ | GENERIC_WRITE).0)
    }

    /// Opens the first present device with the SudoVDA interface, with no
    /// data access (desired access 0). All SudoVDA IOCTLs use
    /// `FILE_ANY_ACCESS`, which the I/O manager sends for any handle to the
    /// device ("Defining I/O Control Codes", Microsoft Learn), so the
    /// queries work through this handle.
    pub fn open_read_only() -> Result<Self, Error> {
        Self::open_with(0)
    }

    fn open_with(access: u32) -> Result<Self, Error> {
        let path = interface_paths()?
            .into_iter()
            .next()
            .ok_or(Error::NotFound)?;
        let wide = wide(&path);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }?;
        Ok(Self { handle, path })
    }

    /// Device interface path of the open device.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Sends a keep-alive. Resets the watchdog for all virtual displays.
    pub fn ping(&self) -> Result<(), Error> {
        self.ioctl(IOCTL_DRIVER_PING, None, None).map(|_| ())
    }

    /// Reads the protocol version. Resets the watchdog as a side effect.
    pub fn protocol_version(&self) -> Result<ProtocolVersion, Error> {
        self.read_struct(IOCTL_GET_PROTOCOL_VERSION)
    }

    /// Reads timeout and countdown. Does not reset the watchdog.
    pub fn watchdog(&self) -> Result<WatchdogOut, Error> {
        self.read_struct(IOCTL_GET_WATCHDOG)
    }

    /// Adds a virtual display. The driver removes it when the watchdog
    /// expires, so the caller must [`ping`](Self::ping) at least once per
    /// timeout. Needs a handle from [`open`](Self::open).
    #[cfg(feature = "host-test")]
    pub fn add_display(&self, params: &crate::AddParams) -> Result<crate::AddOut, Error> {
        let mut out = crate::AddOut::default();
        let got = self.ioctl(
            crate::IOCTL_ADD_VIRTUAL_DISPLAY,
            Some((
                (params as *const crate::AddParams).cast(),
                size_of::<crate::AddParams>() as u32,
            )),
            Some((
                (&mut out as *mut crate::AddOut).cast(),
                size_of::<crate::AddOut>() as u32,
            )),
        )?;
        if (got as usize) < size_of::<crate::AddOut>() {
            return Err(Error::ShortReply {
                expected: size_of::<crate::AddOut>(),
                got,
            });
        }
        Ok(out)
    }

    /// Removes the virtual display with this monitor GUID.
    #[cfg(feature = "host-test")]
    pub fn remove_display(&self, params: &crate::RemoveParams) -> Result<(), Error> {
        self.ioctl(
            crate::IOCTL_REMOVE_VIRTUAL_DISPLAY,
            Some((
                (params as *const crate::RemoveParams).cast(),
                size_of::<crate::RemoveParams>() as u32,
            )),
            None,
        )
        .map(|_| ())
    }

    fn read_struct<T: Default + Copy>(&self, code: u32) -> Result<T, Error> {
        let mut out = T::default();
        let got = self.ioctl(
            code,
            None,
            Some(((&mut out as *mut T).cast(), size_of::<T>() as u32)),
        )?;
        if (got as usize) < size_of::<T>() {
            return Err(Error::ShortReply {
                expected: size_of::<T>(),
                got,
            });
        }
        Ok(out)
    }

    fn ioctl(
        &self,
        code: u32,
        input: Option<(*const c_void, u32)>,
        output: Option<(*mut c_void, u32)>,
    ) -> Result<u32, Error> {
        let mut returned = 0u32;
        let (in_ptr, in_len) = input.map_or((None, 0), |(p, n)| (Some(p), n));
        let (out_ptr, out_len) = output.map_or((None, 0), |(p, n)| (Some(p), n));
        // SAFETY: the buffers come from live references sized by the caller;
        // the call is synchronous (no OVERLAPPED).
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                in_ptr,
                in_len,
                out_ptr,
                out_len,
                Some(&mut returned),
                None,
            )
        }?;
        Ok(returned)
    }
}

// SAFETY: the handle is a plain kernel file handle used with synchronous
// DeviceIoControl only. Any thread may use or close it.
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// Paths of all present devices with the SudoVDA interface.
pub fn interface_paths() -> Result<Vec<String>, Error> {
    let guid = GUID::from_values(
        INTERFACE_GUID.data1,
        INTERFACE_GUID.data2,
        INTERFACE_GUID.data3,
        INTERFACE_GUID.data4,
    );
    read_cm_list(
        // SAFETY: valid out pointer and GUID.
        |len| unsafe {
            CM_Get_Device_Interface_List_SizeW(
                len,
                &guid,
                PCWSTR::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        },
        // SAFETY: `buf` is a live, writable buffer of the given length.
        |buf| unsafe {
            CM_Get_Device_Interface_ListW(
                &guid,
                PCWSTR::null(),
                buf,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        },
    )
}

/// Reads a configuration manager string list: `size` stores the length in
/// characters, `list` fills the buffer. An entry can arrive between the
/// two calls; then `list` returns `CR_BUFFER_SMALL` and both run again.
pub fn read_cm_list(
    mut size: impl FnMut(&mut u32) -> CONFIGRET,
    mut list: impl FnMut(&mut [u16]) -> CONFIGRET,
) -> Result<Vec<String>, Error> {
    loop {
        let mut len = 0u32;
        let cr = size(&mut len);
        if cr != CR_SUCCESS {
            return Err(Error::ConfigManager(cr.0));
        }
        let mut buf = vec![0u16; (len as usize).max(1)];
        let cr = list(&mut buf);
        if cr == CR_BUFFER_SMALL {
            continue;
        }
        if cr != CR_SUCCESS {
            return Err(Error::ConfigManager(cr.0));
        }
        return Ok(split_multi_sz(&buf));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Devices::DeviceAndDriverInstallation::CR_FAILURE;

    #[test]
    fn read_cm_list_retries_when_the_list_grows() {
        let entries = ["a", "bc"];
        let mut calls = 0;
        let got = read_cm_list(
            // The first size misses the second entry.
            |len| {
                calls += 1;
                *len = if calls == 1 { 3 } else { 6 };
                CR_SUCCESS
            },
            |buf| {
                let mut data: Vec<u16> = Vec::new();
                for e in entries {
                    data.extend(wide(e));
                }
                data.push(0);
                if buf.len() < data.len() {
                    return CR_BUFFER_SMALL;
                }
                buf[..data.len()].copy_from_slice(&data);
                CR_SUCCESS
            },
        )
        .unwrap();
        assert_eq!(got, ["a", "bc"]);
    }

    #[test]
    fn read_cm_list_reports_errors() {
        let size_fails = read_cm_list(|_| CR_FAILURE, |_| CR_SUCCESS);
        assert!(matches!(size_fails, Err(Error::ConfigManager(c)) if c == CR_FAILURE.0));
        let list_fails = read_cm_list(|_| CR_SUCCESS, |_| CR_FAILURE);
        assert!(matches!(list_fails, Err(Error::ConfigManager(c)) if c == CR_FAILURE.0));
    }
}
