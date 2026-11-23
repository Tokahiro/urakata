//! Tray icon of `urakata-watch.exe` (Windows only). The main thread runs
//! a hidden window with its message loop and the icon; the watcher loop
//! (`watcher::watch_with`) runs on a second thread. The loop sends events
//! to the window with `PostMessageW`; the menu sends requests back through
//! a `watcher::Remote`. The program ends when the watcher loop ends.
//!
//! The icon appears after the loop's first event, so a second watcher that
//! stops at the single-instance lock shows no icon. A hidden top-level
//! window (not a message-only one) gets `TaskbarCreated`, so the icon comes
//! back after Explorer restarts.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFY_ICON_DATA_FLAGS,
    NOTIFY_ICON_INFOTIP_FLAGS, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    HICON, IDI_APPLICATION, IMAGE_ICON, KillTimer, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, SetTimer, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

use crate::settings;
use crate::watcher::{self, Event, Remote};

/// Messages from the icon.
const WM_TRAY: u32 = WM_APP + 1;
/// The watcher loop has news in `Shared`.
const WM_WATCHER: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
/// `NIN_KEYSELECT` (`NIN_SELECT | NINF_KEY`), which the crate does not have.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const TIMER_ID: usize = 1;
/// The settings file is read this often, so that hiding or showing the
/// icon in the settings window applies within some seconds.
const SETTINGS_EVERY_MS: u32 = 5000;

const MENU_OPEN: usize = 1;
const MENU_RESTORE: usize = 2;
const MENU_STOP: usize = 3;

/// Icon numbers in `assets/logo/icons.rc`.
const ICON_ACTIVE: usize = 1;
const ICON_IDLE: usize = 2;

/// What the watcher thread hands to the window thread.
#[derive(Default)]
struct Shared {
    events: Vec<Event>,
    ended: bool,
}

struct Tray {
    hwnd: HWND,
    remote: Remote,
    shared: Arc<Mutex<Shared>>,
    /// Curtain closed: no stream runs.
    icon_idle: HICON,
    /// Curtain open: a stream runs.
    icon_active: HICON,
    /// `TaskbarCreated`.
    taskbar_created: u32,
    /// `None` until the loop's first event.
    stream: Option<bool>,
    shown: bool,
}

thread_local! {
    /// The tray of the window thread; the window procedure reads it.
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Runs the watcher with a tray icon. Without a window (it could not be
/// made) the watcher runs without one.
pub fn run() -> Result<(), String> {
    dark_menus();
    let (remote, inbox) = watcher::channel();
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            watcher::log_note(&format!("No tray icon: {e}."));
            return watcher::watch_with(inbox, |_| {});
        }
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    // A HWND is a pointer and not `Send`; pass its value.
    let raw = hwnd.0 as usize;
    let thread = std::thread::spawn({
        let shared = shared.clone();
        move || {
            let post = || {
                // SAFETY: the window lives until it gets `ended`.
                let _ = unsafe {
                    PostMessageW(Some(HWND(raw as *mut _)), WM_WATCHER, WPARAM(0), LPARAM(0))
                };
            };
            // A panic must still end the window, or the process would
            // keep the single-instance lock without watching.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                watcher::watch_with(inbox, |event| {
                    lock(&shared).events.push(event);
                    post();
                })
            }))
            .unwrap_or_else(|_| {
                watcher::log_note("Stopped: the watcher thread failed unexpectedly.");
                Err("the watcher thread failed unexpectedly".into())
            });
            lock(&shared).ended = true;
            post();
            result
        }
    });
    TRAY.with(|t| {
        *t.borrow_mut() = Some(Tray {
            hwnd,
            remote,
            shared,
            icon_idle: load_icon(ICON_IDLE),
            icon_active: load_icon(ICON_ACTIVE),
            // SAFETY: constant string.
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            stream: None,
            shown: false,
        });
    });
    // SAFETY: own window; a failed timer only stops the settings reads.
    unsafe { SetTimer(Some(hwnd), TIMER_ID, SETTINGS_EVERY_MS, None) };
    let mut msg = MSG::default();
    // SAFETY: standard message loop of this thread.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if let Some(mut tray) = TRAY.with(|t| t.borrow_mut().take()) {
        tray.set_shown(false);
        // The loop also ends on WM_QUIT from outside; the watcher must end
        // too, or `join` waits for ever.
        tray.remote.stop();
        // SAFETY: the icons are not in use any more.
        unsafe {
            let _ = DestroyIcon(tray.icon_idle);
            let _ = DestroyIcon(tray.icon_active);
        }
    }
    thread
        .join()
        .unwrap_or_else(|_| Err("the watcher thread failed unexpectedly".into()))
}

/// Makes the tray menu dark, like the settings window. Windows has no
/// public call for this: `uxtheme.dll` exports `SetPreferredAppMode` as
/// ordinal 135 and `FlushMenuThemes` as ordinal 136 (Windows 10 1903 and
/// later; Explorer and Notepad++ use them too). If they are missing, the
/// menu stays light.
fn dark_menus() {
    const SET_PREFERRED_APP_MODE: usize = 135;
    const FLUSH_MENU_THEMES: usize = 136;
    const FORCE_DARK: i32 = 2;
    // SAFETY: uxtheme.dll stays loaded for the life of the process, and it
    // comes from System32 only. The signatures hold from build 18362 on;
    // the installer needs 19041. On 1809, 135 is AllowDarkModeForApp(bool):
    // the same x64 call, so the menu only stays light.
    unsafe {
        let Ok(uxtheme) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
        else {
            return;
        };
        let set_mode = GetProcAddress(uxtheme, PCSTR(SET_PREFERRED_APP_MODE as *const u8));
        let flush = GetProcAddress(uxtheme, PCSTR(FLUSH_MENU_THEMES as *const u8));
        if let (Some(set_mode), Some(flush)) = (set_mode, flush) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            set_mode(FORCE_DARK);
            flush();
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn create_window() -> Result<HWND, String> {
    let class = w!("urakata-watch-tray");
    // SAFETY: plain calls; the class lives for the whole process.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| format!("no module handle: {e}"))?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(format!(
                "cannot register the window class: {}",
                windows::core::Error::from_thread()
            ));
        }
        // Never shown.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            // Not "urakata": the settings window finds itself by that title.
            w!("urakata tray"),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("cannot create the window: {e}"))
    }
}

/// Loads an icon of this program in the size of the notification area.
fn load_icon(number: usize) -> HICON {
    // SAFETY: plain calls; a small number in place of a name is
    // MAKEINTRESOURCE.
    unsafe {
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        GetModuleHandleW(None)
            .and_then(|module| {
                LoadImageW(
                    Some(module.into()),
                    PCWSTR(number as *const u16),
                    IMAGE_ICON,
                    cx,
                    cy,
                    LR_DEFAULTCOLOR,
                )
            })
            .map(|handle| HICON(handle.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DESTROY {
        // SAFETY: plain call on the window thread.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // `try_borrow_mut`: a menu runs its own message loop, so this can be
    // called again while the tray is borrowed. Such calls get the default.
    let handled = TRAY.with(|t| {
        let Ok(mut slot) = t.try_borrow_mut() else {
            return false;
        };
        let Some(tray) = slot.as_mut() else {
            return false;
        };
        tray.message(msg, lparam)
    });
    if handled {
        return LRESULT(0);
    }
    // SAFETY: default handling for this window's message.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Tray {
    /// Handles a message; `false` for the default handling.
    fn message(&mut self, msg: u32, lparam: LPARAM) -> bool {
        match msg {
            // `taskkill` without /F and the Restart Manager send WM_CLOSE:
            // stop the watcher, and the window ends with it.
            WM_CLOSE => self.remote.stop(),
            WM_WATCHER => self.watcher_news(),
            WM_TIMER => self.sync_icon(),
            // With version 4 the low word of `lparam` is the event.
            WM_TRAY => match (lparam.0 & 0xffff) as u32 {
                WM_CONTEXTMENU => self.menu(),
                NIN_SELECT | NIN_KEYSELECT => self.open_settings(),
                _ => {}
            },
            m if m == self.taskbar_created && m != 0 => {
                // Explorer started again and lost the icon.
                self.shown = false;
                self.sync_icon();
            }
            _ => return false,
        }
        true
    }

    fn watcher_news(&mut self) {
        let (events, ended) = {
            let mut shared = lock(&self.shared);
            (std::mem::take(&mut shared.events), shared.ended)
        };
        for event in events {
            match event {
                Event::Watching { stream } => {
                    self.stream = Some(stream);
                    self.sync_icon();
                }
                // The restore text names the profile path; the details are
                // in watch.log.
                Event::Restored(Ok(_)) => self.balloon(
                    "Displays restored",
                    "The displays are on in the layout Windows stored.",
                    false,
                ),
                Event::Restored(Err(_)) => self.balloon(
                    "Restore did not finish",
                    "Some displays were not restored. See watch.log in the log folder.",
                    true,
                ),
            }
        }
        if ended {
            self.set_shown(false);
            // SAFETY: own window and timer; WM_DESTROY ends the loop.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    /// Shows or hides the icon as the loop state and the setting say, and
    /// keeps its tip current.
    fn sync_icon(&mut self) {
        let wanted = self.stream.is_some()
            && settings::settings_path()
                .and_then(|p| settings::load(&p))
                // The watcher uses the defaults for a damaged file too.
                .map_or(true, |s| s.tray_icon);
        if wanted && self.shown {
            // The icon is gone when Explorer restarted while the menu was
            // open (its TaskbarCreated got the default handling).
            if !self.notify(NIM_MODIFY, NIF_ICON | NIF_TIP | NIF_SHOWTIP) {
                self.shown = false;
                self.set_shown(true);
            }
        } else {
            // Adding fails while Explorer is not ready (early at logon);
            // the timer tries again.
            self.set_shown(wanted);
        }
    }

    fn set_shown(&mut self, on: bool) {
        if on == self.shown {
            return;
        }
        if on {
            if self.notify(NIM_ADD, NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP) {
                self.notify(NIM_SETVERSION, NOTIFY_ICON_DATA_FLAGS(0));
                self.shown = true;
            }
        } else {
            self.notify(NIM_DELETE, NOTIFY_ICON_DATA_FLAGS(0));
            self.shown = false;
        }
    }

    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            uFlags: flags,
            uCallbackMessage: WM_TRAY,
            hIcon: if self.stream == Some(true) {
                self.icon_active
            } else {
                self.icon_idle
            },
            ..Default::default()
        };
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        copy_wide(&mut data.szTip, &format!("urakata\n{}", self.state_line()));
        data
    }

    fn notify(&self, message: NOTIFY_ICON_MESSAGE, flags: NOTIFY_ICON_DATA_FLAGS) -> bool {
        let data = self.data(flags);
        // SAFETY: `data` is complete and lives for the call.
        unsafe { Shell_NotifyIconW(message, &data) }.as_bool()
    }

    /// A notification from the icon. Nothing when the icon is hidden.
    fn balloon(&self, title: &str, text: &str, warning: bool) {
        if !self.shown {
            return;
        }
        let mut data = self.data(NIF_INFO);
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        data.dwInfoFlags =
            if warning { NIIF_WARNING } else { NIIF_INFO } as NOTIFY_ICON_INFOTIP_FLAGS;
        // SAFETY: as in `notify`.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn state_line(&self) -> &'static str {
        state_line(self.stream)
    }

    fn menu(&mut self) {
        let mut point = POINT::default();
        // SAFETY: menu calls on the window thread; the menu is destroyed
        // below. SetForegroundWindow and the WM_NULL post make the menu
        // close on a click outside it (documented for TrackPopupMenu).
        let chosen = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let state = HSTRING::from(self.state_line());
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(state.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open settings"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_RESTORE, w!("Restore displays"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_STOP, w!("Stop watcher"));
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow(self.hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            chosen.0 as usize
        };
        match chosen {
            MENU_OPEN => self.open_settings(),
            MENU_RESTORE => self.remote.restore(),
            MENU_STOP => self.remote.stop(),
            _ => {}
        }
        // The menu's own message loop gave the watcher's posts the default
        // handling; their news is still in `Shared`.
        self.watcher_news();
    }

    /// Starts `urakata-app.exe` next to this program.
    fn open_settings(&self) {
        let result = std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| {
                let app = exe.with_file_name("urakata-app.exe");
                std::process::Command::new(&app)
                    .spawn()
                    .map(drop)
                    .map_err(|e| format!("cannot start {}: {e}", app.display()))
            });
        if let Err(e) = result {
            watcher::log_note(&format!("Tray icon: {e}"));
            self.balloon("Cannot open the settings", &e, true);
        }
    }
}

/// The first line of the menu and the icon tip.
fn state_line(stream: Option<bool>) -> &'static str {
    match stream {
        None => "Starting",
        Some(false) => "Watching. No stream runs.",
        Some(true) => "Watching. A stream runs.",
    }
}

/// Copies `text` into a fixed UTF-16 field, cut to fit, with the ending 0.
fn copy_wide(field: &mut [u16], text: &str) {
    let Some(room) = field.len().checked_sub(1) else {
        return;
    };
    let mut n = 0;
    for unit in text.encode_utf16().take(room) {
        field[n] = unit;
        n += 1;
    }
    // Do not end on the first half of a surrogate pair.
    if n > 0 && (0xD800..0xDC00).contains(&field[n - 1]) {
        n -= 1;
    }
    field[n] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_text_is_cut_and_ended() {
        let mut field = [0xffff_u16; 4];
        copy_wide(&mut field, "abcdef");
        assert_eq!(field, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        let mut field = [0xffff_u16; 4];
        copy_wide(&mut field, "ab");
        assert_eq!(&field[..3], &[b'a' as u16, b'b' as u16, 0]);
        // "a" + U+1F600 (two units) does not fit in 3 units: the pair is
        // dropped whole.
        let mut field = [0xffff_u16; 3];
        copy_wide(&mut field, "a\u{1F600}");
        assert_eq!(field[..2], [b'a' as u16, 0]);
    }

    #[test]
    fn state_lines() {
        assert_eq!(state_line(Some(false)), "Watching. No stream runs.");
        assert_eq!(state_line(Some(true)), "Watching. A stream runs.");
    }
}
