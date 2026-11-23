//! The settings window (Windows only). Reads and changes go to worker
//! threads, so that the window never hangs on a slow Windows call.

use std::cell::Cell;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};
use urakata::report::Report;
use urakata::settings::{self, MAX_GUARD_REAPPLY, MAX_GUARD_S, MAX_START_DELAY_S, Settings};
use urakata::{data_dir, display, driver, status, update, watcher};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, GetLastError, HWND, WAIT_TIMEOUT,
};
use windows::Win32::Graphics::Gdi::{
    DESKTOPHORZRES, DESKTOPVERTRES, GetDC, GetDeviceCaps, ReleaseDC,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    CreateMutexW, GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetForegroundWindow, GetWindowThreadProcessId, SW_HIDE, SW_RESTORE,
    SetForegroundWindow, ShowWindow,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::pad::{self, Pad};
use crate::view;

slint::include_modules!();

/// The status is read again this often while the window is open.
const REFRESH: Duration = Duration::from_secs(10);
/// How long Start waits for the new watcher's lock.
const START_WAIT: Duration = Duration::from_secs(5);
/// How long the window waits for an elevated `driver repair`. It waits
/// some seconds for the device itself; a driver install that hangs longer
/// must not keep the window busy for ever.
const REPAIR_WAIT_MS: u32 = 10 * 60 * 1000;
/// How long Stop waits for the watcher to end. It ends after the display
/// change it runs, which can take some seconds.
const STOP_WAIT: Duration = Duration::from_secs(15);
/// The controller is read this often.
const PAD_POLL: Duration = Duration::from_millis(30);
/// With no controller, the four XInput slots are looked at this often
/// (a read of an empty slot is slow).
const PAD_SCAN: Duration = Duration::from_secs(1);
/// The argument that the Steam entry passes (`urakata steam-entry --add`).
const BIG_PICTURE_ARG: &str = "--big-picture";
/// Slint's winit backend takes the scale from this variable.
const SCALE_VAR: &str = "SLINT_SCALE_FACTOR";

/// The settings as saved; `None` when the file could not be used, so that
/// Save stays on until a save repairs it.
type Saved = Rc<Cell<Option<Settings>>>;

/// Which parts a status read reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    /// The status report.
    Status,
    /// The report and the driver state. The driver read opens each
    /// `oem*.inf` in the driver store, so the timer does not ask for it.
    WithDriver,
}

thread_local! {
    /// A read was asked for while another one ran (window thread only).
    static READ_AGAIN: Cell<Option<Read>> = const { Cell::new(None) };
}

pub fn run() -> ExitCode {
    if already_open() {
        return ExitCode::SUCCESS;
    }
    let big_picture = big_picture();
    if big_picture {
        let (w, h) = screen_size();
        // SAFETY: no other thread runs yet. The winit backend reads it
        // when the window is made.
        unsafe { std::env::set_var(SCALE_VAR, view::big_picture_scale(w, h).to_string()) };
    }
    let ui = match AppWindow::new() {
        Ok(ui) => ui,
        Err(e) => {
            eprintln!("urakata-app: cannot open the window: {e}");
            return ExitCode::FAILURE;
        }
    };
    let saved: Saved = Rc::new(Cell::new(None));
    ui.set_max_start_delay(MAX_START_DELAY_S as i32);
    ui.set_max_guard_time(MAX_GUARD_S as i32);
    ui.set_max_guard_reapply(MAX_GUARD_REAPPLY as i32);
    ui.set_version(update::CURRENT.into());
    ui.set_driver_line("Reading…".into());
    load_settings(&ui, &saved);
    wire(&ui, &saved);
    if big_picture {
        ui.global::<Kido>().set_big_picture(true);
        ui.window().set_fullscreen(true);
        // Show the focus at once: a controller has no pointer to start with.
        ui.invoke_go(ui.get_page(), true);
    }
    let _pad = read_controller(&ui);
    refresh(&ui, Read::WithDriver);
    let timer = Timer::default();
    let weak = ui.as_weak();
    timer.start(TimerMode::Repeated, REFRESH, move || {
        // An action reads again when it ends.
        if let Some(ui) = weak.upgrade()
            && !ui.get_reading()
            && !ui.get_busy()
        {
            refresh(&ui, Read::Status);
        }
    });
    match ui.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("urakata-app: {e}");
            ExitCode::FAILURE
        }
    }
}

fn wire(ui: &AppWindow, saved: &Saved) {
    ui.on_move_focus({
        let weak = ui.as_weak();
        move |by| {
            let weak = weak.clone();
            // Not inside the key handler that asked for it.
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = weak.upgrade() {
                    press(&ui, if by < 0 { Key::Backtab } else { Key::Tab });
                }
            });
        }
    });
    ui.on_close_window({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                // The last window hidden ends `run`.
                let _ = ui.hide();
            }
        }
    });
    ui.on_refresh({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, Read::WithDriver);
            }
        }
    });
    ui.on_start_watcher({
        let weak = ui.as_weak();
        move || {
            action(&weak, || {
                let exe = watcher::watch_exe()?;
                Command::new(&exe)
                    // The Big Picture scale must not reach the watcher, or
                    // the windows that its tray opens on the desktop.
                    .env_remove(SCALE_VAR)
                    .spawn()
                    .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
                if !wait_for_watcher(true, START_WAIT) {
                    return Err(
                        "The watcher did not start. See watch.log in the log folder.".into(),
                    );
                }
                Ok("The watcher was started.".into())
            })
        }
    });
    ui.on_stop_watcher({
        let weak = ui.as_weak();
        move || {
            action(&weak, || {
                if !watcher::stop_running()? {
                    return Err(
                        "The running watcher is from an older urakata and cannot be \
                                stopped from here. End urakata-watch.exe in Task Manager, \
                                or log off and on."
                            .into(),
                    );
                }
                if !wait_for_watcher(false, STOP_WAIT) {
                    return Err(
                        "Stop was sent, but the watcher still runs. It may be in the \
                                middle of a display change; read again in a moment."
                            .into(),
                    );
                }
                Ok("The watcher was stopped.".into())
            })
        }
    });
    ui.on_autostart_toggled({
        let weak = ui.as_weak();
        move |on| {
            action(&weak, move || {
                let text = if on {
                    watcher::install()?
                } else {
                    watcher::uninstall()?
                };
                // The console text names commands; one line is enough here.
                Ok(text.lines().next().unwrap_or_default().to_string())
            })
        }
    });
    ui.on_settings_edited({
        let weak = ui.as_weak();
        let saved = saved.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_settings_dirty(saved.get() != Some(edited(&ui)));
            }
        }
    });
    ui.on_default_settings({
        let weak = ui.as_weak();
        let saved = saved.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                show_settings(&ui, Settings::default());
                ui.set_settings_dirty(saved.get() != Some(Settings::default()));
            }
        }
    });
    ui.on_save_settings({
        let weak = ui.as_weak();
        let saved = saved.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let s = edited(&ui);
            match settings::settings_path().and_then(|p| settings::save(&p, &s)) {
                Ok(()) => {
                    saved.set(Some(s));
                    show_settings(&ui, s);
                    ui.set_settings_dirty(false);
                    notice(&ui, "Settings saved.", false);
                }
                Err(e) => notice(&ui, &format!("Cannot save the settings: {e}"), true),
            }
        }
    });
    ui.on_restore_displays({
        let weak = ui.as_weak();
        move || {
            action(&weak, || {
                let outcome = display::restore(false)?;
                if outcome.partial {
                    Err(outcome.text)
                } else {
                    Ok(outcome.text)
                }
            })
        }
    });
    ui.on_repair_driver({
        let weak = ui.as_weak();
        move || {
            // The UAC prompt needs an owner window to come to the front;
            // the button was just pressed, so this window is in front.
            // SAFETY: no arguments.
            let owner = unsafe { GetForegroundWindow() }.0 as usize;
            action(&weak, move || repair_driver(owner))
        }
    });
    ui.on_check_update({
        let weak = ui.as_weak();
        move || action(&weak, || Ok(view::update_text(update::check()?.as_ref())))
    });
    ui.on_open_release_page({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Err(e) = Command::new("explorer.exe")
                .arg(update::RELEASE_PAGE)
                .spawn()
            {
                notice(
                    &ui,
                    &format!("Cannot open {}: {e}", update::RELEASE_PAGE),
                    true,
                );
            }
        }
    });
    ui.on_open_folder({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let result = data_dir().and_then(|dir| {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                Command::new("explorer.exe")
                    .arg(&dir)
                    .spawn()
                    .map(drop)
                    .map_err(|e| format!("cannot open {}: {e}", dir.display()))
            });
            if let Err(e) = result {
                notice(&ui, &e, true);
            }
        }
    });
}

/// Started from the Steam entry, which passes `--big-picture`.
/// Steam passes it from the desktop library too.
fn big_picture() -> bool {
    std::env::args().skip(1).any(|a| a == BIG_PICTURE_ARG)
}

/// The primary screen in physical pixels, whatever DPI awareness the
/// process has yet. During a stream it is the stream's display.
fn screen_size() -> (u32, u32) {
    // SAFETY: the screen DC is released below.
    unsafe {
        let dc = GetDC(None);
        let w = GetDeviceCaps(Some(dc), DESKTOPHORZRES);
        let h = GetDeviceCaps(Some(dc), DESKTOPVERTRES);
        ReleaseDC(None, dc);
        (w.max(0) as u32, h.max(0) as u32)
    }
}

/// Reads the XInput controllers while the window is in front and turns
/// their buttons into window actions (see `pad.rs`). Steam gives a
/// streamed controller to the PC as an XInput controller; another pad may
/// hold a lower slot, so all connected slots count as one controller. The
/// timer stops when the returned value is dropped.
fn read_controller(ui: &AppWindow) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let mut pad = Pad::default();
    let mut slots: Vec<u32> = Vec::new();
    let mut scan_at = Instant::now();
    let mut last = Instant::now();
    // Whether the sidebar had the focus when the held direction was
    // pressed: a repeat stops where the focus crosses between the sidebar
    // and the page, so that a held Down does not run on through the pages.
    let mut held_on_rail = false;
    timer.start(TimerMode::Repeated, PAD_POLL, move || {
        let Some(ui) = weak.upgrade() else { return };
        let now = Instant::now();
        let dt = now - last;
        last = now;
        if !in_front() {
            pad.reset();
            return;
        }
        if now >= scan_at {
            scan_at = now + PAD_SCAN;
            for i in 0..4 {
                if !slots.contains(&i) && xinput(i).is_some() {
                    slots.push(i);
                }
            }
        }
        let mut buttons = 0u16;
        let (mut x, mut y) = (0i16, 0i16);
        let mut lost = false;
        slots.retain(|&i| match xinput(i) {
            Some(s) => {
                let g = s.Gamepad;
                buttons |= g.wButtons.0;
                // Per axis, the stick that is pushed furthest.
                if g.sThumbLX.unsigned_abs() > x.unsigned_abs() {
                    x = g.sThumbLX;
                }
                if g.sThumbLY.unsigned_abs() > y.unsigned_abs() {
                    y = g.sThumbLY;
                }
                true
            }
            None => {
                lost = true;
                false
            }
        });
        if lost {
            // A button of the lost pad must not look released.
            pad.reset();
        }
        if slots.is_empty() {
            return;
        }
        for action in pad.step(buttons, x, y, dt) {
            let direction = matches!(
                action,
                pad::Action::Up | pad::Action::Down | pad::Action::Left | pad::Action::Right
            );
            if direction {
                let on_rail = ui.invoke_rail_focused();
                if !pad.repeated() {
                    held_on_rail = on_rail;
                } else if on_rail != held_on_rail {
                    continue;
                }
            }
            act(&ui, action);
        }
    });
    timer
}

fn xinput(slot: u32) -> Option<XINPUT_STATE> {
    let mut state = XINPUT_STATE::default();
    // SAFETY: `state` outlives the call.
    (unsafe { XInputGetState(slot, &mut state) } == 0).then_some(state)
}

/// Whether the window in front is one of this process: a controller used
/// in Big Picture or a game must not work this window.
fn in_front() -> bool {
    // SAFETY: plain calls.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid == std::process::id()
    }
}

fn act(ui: &AppWindow, action: pad::Action) {
    use pad::Action;
    match action {
        Action::Up => press(ui, Key::UpArrow),
        Action::Down => press(ui, Key::DownArrow),
        Action::Left => press(ui, Key::LeftArrow),
        Action::Right => press(ui, Key::RightArrow),
        Action::Select => press(ui, Key::Space),
        Action::Back => ui.invoke_back(),
        Action::PrevPage => ui.invoke_go(ui.get_page() - 1, true),
        Action::NextPage => ui.invoke_go(ui.get_page() + 1, true),
    }
}

/// Sends a key press and release to the window, as a keyboard would.
fn press(ui: &AppWindow, key: Key) {
    let text = SharedString::from(key);
    let window = ui.window();
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

/// Runs `urakata driver repair` elevated (Windows asks for admin rights)
/// and waits for it. Its output goes to `driver-repair.log`, because the
/// elevated program has no console that this window could read.
fn repair_driver(owner: usize) -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot find this program: {e}"))?
        .with_file_name("urakata.exe");
    if !exe.is_file() {
        return Err(format!("{} is missing", exe.display()));
    }
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let log = dir.join("driver-repair.log");
    // An old log must not be read as the result of this run.
    if let Err(e) = std::fs::remove_file(&log)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("cannot remove {}: {e}", log.display()));
    }
    let params = format!("driver repair --log \"{}\"", log.display());
    let code = run_elevated(owner, &exe, &params)?;
    let text = std::fs::read_to_string(&log).ok();
    view::repair_result(code, text.as_deref())
}

/// Starts `exe` with the `runas` verb, waits for it and returns its exit
/// code. Runs on a worker thread. `owner` is the window (as a number,
/// because a HWND is not `Send`) that the UAC prompt belongs to.
fn run_elevated(owner: usize, exe: &Path, params: &str) -> Result<u32, String> {
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: HWND(owner as *mut _),
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: ShellExecuteEx wants COM on the calling thread (Microsoft's
    // remarks); this worker thread ends after the call. `info` and the
    // strings it points to live until the call returns.
    let started = unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let started = ShellExecuteExW(&mut info);
        if com.is_ok() {
            CoUninitialize();
        }
        started
    };
    if let Err(e) = started {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            "The admin prompt was cancelled. Nothing was changed.".into()
        } else {
            format!("cannot start {}: {e}", exe.display())
        });
    }
    if info.hProcess.is_invalid() {
        return Err(format!("no process handle for {}", exe.display()));
    }
    let mut code = 0u32;
    // SAFETY: the handle comes from ShellExecuteExW (NOCLOSEPROCESS) and
    // is closed here.
    let read = unsafe {
        let read = if WaitForSingleObject(info.hProcess, REPAIR_WAIT_MS) == WAIT_TIMEOUT {
            None
        } else {
            Some(GetExitCodeProcess(info.hProcess, &mut code))
        };
        let _ = CloseHandle(info.hProcess);
        read
    };
    match read {
        None => Err(
            "The repair still runs after 10 minutes. Press \"Read again\" later; \
                     driver-repair.log in the log folder gets its result."
                .into(),
        ),
        Some(r) => r
            .map(|()| code)
            .map_err(|e| format!("cannot read the exit code: {e}")),
    }
}

/// One settings window per user session: two windows would save over each
/// other. A second start brings the open window to the front and ends.
fn already_open() -> bool {
    // SAFETY: plain call with a constant name. The handle stays open
    // until exit, which holds the lock.
    let mutex = unsafe { CreateMutexW(None, false, w!(r"Local\urakata-app")) };
    // SAFETY: read right after the call that set it.
    if mutex.is_err() || unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return false;
    }
    if let Some(hwnd) = open_window() {
        // SAFETY: plain calls; a failure only leaves the window behind.
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    true
}

/// The top-level window titled "urakata" of another `urakata-app.exe`.
/// The title alone is not enough: an Explorer window of a folder named
/// urakata has it too.
fn open_window() -> Option<HWND> {
    let mut after = None;
    loop {
        // SAFETY: plain calls; `after` is a window from the last call.
        let hwnd = unsafe { FindWindowExW(None, after, None, w!("urakata")) }.ok()?;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != std::process::id() && process_is_app(pid) {
            return Some(hwnd);
        }
        after = Some(hwnd);
    }
}

fn process_is_app(pid: u32) -> bool {
    // SAFETY: the handle is closed below; the buffer outlives the call.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        read.is_ok()
            && String::from_utf16_lossy(&buf[..len as usize])
                .to_ascii_lowercase()
                .ends_with("\\urakata-app.exe")
    }
}

/// Waits until the watcher's lock shows the wanted state.
fn wait_for_watcher(running: bool, wait: Duration) -> bool {
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    loop {
        if status::watcher_running().is_ok_and(|r| r == running) {
            return true;
        }
        if waited >= wait {
            return false;
        }
        std::thread::sleep(step);
        waited += step;
    }
}

fn load_settings(ui: &AppWindow, saved: &Saved) {
    if let Ok(dir) = data_dir() {
        ui.set_settings_path(dir.display().to_string().into());
    }
    match settings::settings_path().and_then(|p| settings::load(&p)) {
        Ok(s) => {
            saved.set(Some(s));
            show_settings(ui, s);
        }
        Err(e) => {
            // The watcher uses the defaults for a damaged file too.
            show_settings(ui, Settings::default());
            ui.set_settings_dirty(true);
            // Open on the Settings page, under its Save row, where the fix is.
            ui.set_page(2);
            ui.set_notice_at("settings".into());
            notice(
                ui,
                &format!(
                    "Cannot use the saved settings ({e}). Showing the defaults; save them to repair the file."
                ),
                true,
            );
        }
    }
}

fn show_settings(ui: &AppWindow, s: Settings) {
    ui.set_exclusive(s.exclusive);
    ui.set_start_delay(s.start_delay_s as i32);
    ui.set_guard_time(s.guard_s as i32);
    ui.set_guard_reapply(s.guard_reapply as i32);
    ui.set_tray_icon(s.tray_icon);
}

fn edited(ui: &AppWindow) -> Settings {
    view::settings_from(
        ui.get_exclusive(),
        ui.get_start_delay(),
        ui.get_guard_time(),
        ui.get_guard_reapply(),
        ui.get_tray_icon(),
    )
}

fn notice(ui: &AppWindow, text: &str, error: bool) {
    ui.set_notice(text.trim_end().into());
    ui.set_notice_error(error);
}

/// Runs a change on a worker thread, shows its result and reads the
/// status again.
fn action<F>(weak: &Weak<AppWindow>, work: F)
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let Some(ui) = weak.upgrade() else { return };
    if ui.get_busy() {
        return;
    }
    ui.set_busy(true);
    // The control that started the action. Its result shows there, even
    // if another control or page was used while it ran.
    let at = ui.get_notice_at();
    let weak = weak.clone();
    std::thread::spawn(move || {
        let result = work();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_busy(false);
            ui.set_notice_at(at);
            match result {
                Ok(text) => notice(&ui, &text, false),
                Err(text) => notice(&ui, &text, true),
            }
            refresh(&ui, Read::WithDriver);
        });
    });
}

/// Reads the status on a worker thread and shows it.
fn refresh(ui: &AppWindow, read: Read) {
    if ui.get_reading() {
        // The running read can be older than the change that asks for
        // this one: read again when it ends.
        READ_AGAIN.with(|r| {
            if r.get() != Some(Read::WithDriver) {
                r.set(Some(read));
            }
        });
        return;
    }
    ui.set_reading(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let report = status::collect();
        let driver = (read == Read::WithDriver).then(driver::read_state);
        let at = local_time();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if let Some(state) = &driver {
                show_driver(
                    &ui,
                    view::driver_view(state.as_ref().map_err(String::as_str)),
                );
            }
            show_report(&ui, &report, &at);
        });
    });
}

fn show_driver(ui: &AppWindow, v: view::DriverView) {
    ui.set_driver_line(v.line.into());
    ui.set_driver_details(v.details.into());
    ui.set_driver_button(v.button.into());
    ui.set_driver_can_repair(v.can_repair);
}

fn show_report(ui: &AppWindow, report: &Report, at: &str) {
    let rows: Vec<CheckRow> = view::rows(&report.checks)
        .into_iter()
        .map(|r| CheckRow {
            title: r.title.into(),
            level: r.level.into(),
            message: r.message.into(),
        })
        .collect();
    ui.set_checks(ModelRc::new(VecModel::from(rows)));
    let errors: Vec<SharedString> = report.errors.iter().map(SharedString::from).collect();
    ui.set_errors(ModelRc::new(VecModel::from(errors)));
    ui.set_stream_text(view::stream_text(report.devices.as_deref()).into());
    let setup = &report.setup;
    ui.set_watcher_known(setup.watcher_running.is_some());
    ui.set_watcher_running(setup.watcher_running == Some(true));
    ui.set_autostart_known(setup.autostart.is_some());
    ui.set_autostart(
        setup
            .autostart
            .as_ref()
            .is_some_and(|a| a.command.is_some()),
    );
    ui.set_read_at(at.into());
    ui.set_reading(false);
    if let Some(read) = READ_AGAIN.with(|r| r.take()) {
        refresh(ui, read);
    }
}

fn local_time() -> String {
    // SAFETY: no arguments.
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}
