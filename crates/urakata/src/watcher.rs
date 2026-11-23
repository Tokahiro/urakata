//! `urakata watch` (Windows only): runs `exclusive` when a stream starts
//! and cleans up after it ends. The timing rules are in `watch.rs`.
//!
//! Trigger: device interface notifications for monitors
//! (`CM_Register_Notification` with `GUID_DEVINTERFACE_MONITOR`). Each
//! event only wakes the loop; the loop then reads the SudoVDA monitor
//! list. The watcher's own display changes do not add or remove SudoVDA
//! monitors, so they cannot start a loop.
//!
//! The tray icon of `urakata-watch.exe` (`tray.rs`) runs this loop on its
//! own thread through [`watch_with`], gets [`Event`]s from it and sends
//! requests with a [`Remote`].
//!
//! Second trigger: resume from sleep (`PowerRegisterSuspendResumeNotification`
//! with a callback, no window needed). It starts the guard again during a
//! stream.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0,
    CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::Win32::Devices::Display::GUID_DEVINTERFACE_MONITOR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
};
use windows::core::{PCWSTR, w};

use crate::display;
use crate::layout::{data_dir, session_path};
use crate::settings::{self, Settings, Tracker};
use crate::status::sudovda_monitor_ids;
use crate::watch::{Due, END_RETRY, Job, Ran, Watch, autostart_command};

/// Name of the value in the `Run` key.
pub(crate) const RUN_VALUE: &str = "urakata";
pub(crate) const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Single-instance lock; `status` checks for it.
pub(crate) const LOCK_NAME: PCWSTR = w!(r"Local\urakata-watch");
/// Event that asks the watcher to stop (set by the settings window).
const STOP_NAME: PCWSTR = w!(r"Local\urakata-watch-stop");
/// The log starts again from empty above this size.
const LOG_LIMIT: u64 = 1024 * 1024;

/// Writes each line with the local time to stdout and to
/// `%LOCALAPPDATA%\urakata\watch.log`.
/// Above `LOG_LIMIT` the file moves to `watch.log.old` and starts again.
struct Log {
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
    /// Lines to write before the next rotation try after a failed rename
    /// (for example a viewer holds the file), so that not each line tries.
    rotate_wait: u32,
}

impl Log {
    fn open() -> Self {
        let path = data_dir().ok().map(|d| d.join("watch.log"));
        let mut log = Self {
            path,
            file: None,
            rotate_wait: 0,
        };
        log.reopen();
        log
    }

    fn reopen(&mut self) {
        self.file = None;
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT)
            && std::fs::rename(path, path.with_extension("log.old")).is_err()
        {
            self.rotate_wait = 100;
        }
        self.file = OpenOptions::new().create(true).append(true).open(path).ok();
    }

    fn line(&mut self, text: &str) {
        // SAFETY: no arguments.
        let t = unsafe { GetLocalTime() };
        let stamp = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
        for l in text.lines() {
            let l = format!("{stamp}  {l}\n");
            let _ = std::io::stdout().write_all(l.as_bytes());
            if let Some(f) = &mut self.file {
                let _ = f.write_all(l.as_bytes());
            }
        }
        if self.rotate_wait > 0 {
            self.rotate_wait -= 1;
        } else if self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() > LOG_LIMIT)
        {
            self.reopen();
        }
    }
}

/// What wakes the loop.
enum Wake {
    /// A monitor arrived or went away.
    Monitor,
    /// The PC resumed from sleep.
    Resume,
    /// The stop event was set, or the tray icon asked to stop.
    Stop,
    /// The tray icon asked to restore the displays.
    Restore,
}

/// What the loop tells the tray icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The loop watches. `stream`: SudoVDA displays were present at the
    /// last read. Sent at start and when this changes.
    Watching { stream: bool },
    /// Result of a restore that [`Remote::restore`] asked for.
    Restored(Result<String, String>),
}

/// Sends requests to the loop. The loop runs them in turn with its own
/// jobs, so a restore never runs at the same time as a display change of
/// the watcher.
#[derive(Clone)]
pub struct Remote(Sender<Wake>);

impl Remote {
    pub fn restore(&self) {
        let _ = self.0.send(Wake::Restore);
    }

    pub fn stop(&self) {
        let _ = self.0.send(Wake::Stop);
    }
}

/// The loop's end of the channel that [`channel`] makes.
pub struct Inbox {
    tx: Sender<Wake>,
    rx: Receiver<Wake>,
}

/// A channel to a loop that [`watch_with`] will run.
pub fn channel() -> (Remote, Inbox) {
    let (tx, rx) = std::sync::mpsc::channel();
    (Remote(tx.clone()), Inbox { tx, rx })
}

/// Monitor interface callback: wakes the loop. It runs on a system thread
/// and must return fast.
unsafe extern "system" fn on_monitor(
    _notify: HCMNOTIFICATION,
    context: *const core::ffi::c_void,
    _action: CM_NOTIFY_ACTION,
    _data: *const CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    // SAFETY: `context` is the `Sender` that `watch` keeps alive until it
    // has unregistered this callback.
    let tx = unsafe { &*context.cast::<Sender<Wake>>() };
    let _ = tx.send(Wake::Monitor);
    0 // ERROR_SUCCESS
}

/// Monitor interface notifications, unregistered on drop.
struct Notification {
    handle: HCMNOTIFICATION,
    // Boxed so that its address, given to Windows, stays the same.
    _tx: Box<Sender<Wake>>,
}

impl Notification {
    fn register(tx: Sender<Wake>) -> Result<Self, String> {
        let tx = Box::new(tx);
        let filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                    ClassGuid: GUID_DEVINTERFACE_MONITOR,
                },
            },
            ..Default::default()
        };
        let mut handle = HCMNOTIFICATION::default();
        let context: *const Sender<Wake> = &*tx;
        // SAFETY: the filter is complete; the context lives in `tx`, which
        // the returned value owns until it unregisters.
        let cr = unsafe {
            CM_Register_Notification(&filter, Some(context.cast()), Some(on_monitor), &mut handle)
        };
        if cr != CR_SUCCESS {
            return Err(format!(
                "CM_Register_Notification failed (CONFIGRET 0x{:x})",
                cr.0
            ));
        }
        Ok(Self { handle, _tx: tx })
    }
}

impl Drop for Notification {
    fn drop(&mut self) {
        // SAFETY: registered handle. Windows waits for running callbacks
        // before it returns, so `_tx` is unused after this.
        unsafe {
            let _ = CM_Unregister_Notification(self.handle);
        }
    }
}

/// Power callback: wakes the loop on resume. Windows sends
/// `PBT_APMRESUMEAUTOMATIC` at each wake and `PBT_APMRESUMESUSPEND` when
/// the user is back (input). Physical displays can come on at either
/// (in a test, a timer wake left them off until input), so both count.
unsafe extern "system" fn on_power(
    context: *const core::ffi::c_void,
    kind: u32,
    _setting: *const core::ffi::c_void,
) -> u32 {
    if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
        // SAFETY: `context` is the `Sender` that `PowerNotification` leaked,
        // so it lives until the process ends.
        let tx = unsafe { &*context.cast::<Sender<Wake>>() };
        let _ = tx.send(Wake::Resume);
    }
    0 // ERROR_SUCCESS
}

/// Resume notifications, unregistered on drop. The callback context and
/// parameters are leaked (once per process): the documentation does not
/// say that unregistering waits for a callback that runs already.
struct PowerNotification {
    handle: HPOWERNOTIFY,
}

impl PowerNotification {
    fn register(tx: Sender<Wake>) -> Result<Self, String> {
        let context: *const Sender<Wake> = Box::leak(Box::new(tx));
        let params: &'static DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
            Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(on_power),
                Context: context.cast_mut().cast(),
            }));
        let recipient: *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS = params;
        let mut handle = std::ptr::null_mut();
        // SAFETY: with DEVICE_NOTIFY_CALLBACK the recipient is a pointer to
        // the parameters; they and the context are leaked, so they stay
        // valid.
        let err = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(recipient.cast_mut().cast()),
                &mut handle,
            )
        };
        if err.is_err() {
            return Err(format!(
                "PowerRegisterSuspendResumeNotification failed (error {})",
                err.0
            ));
        }
        Ok(Self {
            handle: HPOWERNOTIFY(handle as isize),
        })
    }
}

impl Drop for PowerNotification {
    fn drop(&mut self) {
        // SAFETY: registered handle.
        unsafe {
            let _ = PowerUnregisterSuspendResumeNotification(self.handle);
        }
    }
}

/// Sends `Wake::Stop` when another program sets the stop event. The
/// thread ends with the process.
fn listen_for_stop(tx: Sender<Wake>) -> Result<(), String> {
    // SAFETY: plain call with a constant name. Auto-reset, not set. The
    // handle stays open until exit, so that the event keeps existing.
    let event = unsafe { CreateEventW(None, false, false, STOP_NAME) }
        .map_err(|e| format!("cannot create the stop event: {e}"))?;
    // A HANDLE is a pointer and not `Send`; pass its value.
    let raw = event.0 as usize;
    std::thread::spawn(move || {
        let event = HANDLE(raw as *mut _);
        // SAFETY: the handle stays open until the process ends.
        while unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
            if tx.send(Wake::Stop).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// Asks a running watcher to stop. `Ok(false)` when no watcher listens
/// (none runs, or one from before the stop event was added).
pub fn stop_running() -> Result<bool, String> {
    // SAFETY: constant name; the handle is closed at once.
    match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, STOP_NAME) } {
        Ok(h) => {
            // SAFETY: handle from the call above.
            let set = unsafe { SetEvent(h) };
            let _ = unsafe { CloseHandle(h) };
            set.map(|()| true)
                .map_err(|e| format!("cannot set the stop event: {e}"))
        }
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(false),
        Err(e) => Err(format!("cannot open the stop event: {e}")),
    }
}

/// `urakata-watch.exe` next to this program.
pub fn watch_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    let watch_exe = exe.with_file_name("urakata-watch.exe");
    if !watch_exe.is_file() {
        return Err(format!(
            "{} is missing; keep it next to {}",
            watch_exe.display(),
            exe.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(watch_exe)
}

/// Reads the settings file. A damaged file gives the defaults; the log
/// gets a line only when the result changed.
fn read_settings(tracker: &mut Tracker, log: &mut Log) -> Settings {
    let read = settings::settings_path().and_then(|p| settings::load(&p));
    let (settings, line) = tracker.update(read);
    if let Some(line) = line {
        log.line(&line);
    }
    settings
}

/// Waits up to `wait` for the stop event; monitor events are ignored.
fn stop_requested(rx: &Receiver<Wake>, wait: Duration) -> bool {
    let end = Instant::now() + wait;
    loop {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(Wake::Stop) => return true,
            // Nothing to restore before the first read.
            Ok(Wake::Monitor | Wake::Resume | Wake::Restore) => {}
            Err(_) => return false,
        }
    }
}

/// Runs the watcher until an error or the stop event stops it.
pub fn watch() -> Result<(), String> {
    let (_remote, inbox) = channel();
    watch_with(inbox, |_| {})
}

/// [`watch`] with requests from `inbox` and events to `on_event`.
pub fn watch_with(inbox: Inbox, on_event: impl FnMut(Event)) -> Result<(), String> {
    // Under the logon start nobody sees stderr, so the log gets each error.
    let mut log = Log::open();
    let result = single_instance().and_then(|()| {
        log.line(&format!(
            "Watching for SudoVDA displays (pid {}).",
            std::process::id()
        ));
        run_loop(&mut log, inbox, on_event)
    });
    if let Err(e) = &result {
        log.line(&format!("Stopped: {e}"));
    }
    result
}

/// One watcher per user session. The lock stays held until exit.
fn single_instance() -> Result<(), String> {
    // SAFETY: plain call with a constant name.
    let mutex = unsafe { CreateMutexW(None, false, LOCK_NAME) };
    // SAFETY: read right after the call that set it.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    match mutex {
        Err(e) => Err(format!("cannot create the single-instance lock: {e}")),
        Ok(_) if exists => Err("another `urakata watch` runs already".into()),
        Ok(_) => Ok(()),
    }
}

fn run_loop(log: &mut Log, inbox: Inbox, mut on_event: impl FnMut(Event)) -> Result<(), String> {
    // Register first, then read the list, so that no change is missed
    // between the two (Microsoft's documented order).
    let Inbox { tx, rx } = inbox;
    let _notification = Notification::register(tx.clone())?;
    // Without it the watcher still works; only the guard after resume is
    // missing.
    let _power = PowerNotification::register(tx.clone())
        .map_err(|e| log.line(&format!("No guard after sleep: {e}.")))
        .ok();
    listen_for_stop(tx)?;
    let mut tracker = Tracker::default();
    // At logon the device list can be incomplete for a while.
    // Try for about one minute, then stop with the error.
    let mut tries = 0;
    let present = loop {
        match sudovda_monitor_ids() {
            Ok(ids) => break ids,
            Err(e) if tries < 12 => {
                log.line(&format!(
                    "Cannot read the SudoVDA monitors: {e}. Trying again."
                ));
                tries += 1;
                if stop_requested(&rx, END_RETRY) {
                    log.line("Stopped on request.");
                    return Ok(());
                }
            }
            Err(e) => return Err(format!("cannot read the SudoVDA monitors: {e}")),
        }
    };
    if !present.is_empty() {
        log.line(&format!(
            "A stream runs already ({}). Leaving its layout as it is.",
            present.join(", ")
        ));
    }
    let mut w = Watch::new(present, Instant::now());
    w.set_settings(read_settings(&mut tracker, log));
    let mut streaming = w.streaming();
    on_event(Event::Watching { stream: streaming });

    loop {
        let wait = w.deadline().saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Wake::Monitor) => {
                w.event(Instant::now());
                continue;
            }
            Ok(Wake::Resume) => {
                w.resumed(Instant::now());
                continue;
            }
            Ok(Wake::Stop) => {
                log.line("Stopped on request.");
                return Ok(());
            }
            Ok(Wake::Restore) => {
                on_event(Event::Restored(restore_now(log)));
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the notification channel closed".into());
            }
        }
        let now = Instant::now();
        match w.due(now) {
            Due::Read => match sudovda_monitor_ids() {
                Ok(ids) => {
                    // Read before `seen`, so that a new stream uses them.
                    w.set_settings(read_settings(&mut tracker, log));
                    if let Some(msg) = w.seen(ids, now) {
                        log.line(&msg);
                    }
                    if w.streaming() != streaming {
                        streaming = w.streaming();
                        on_event(Event::Watching { stream: streaming });
                    }
                }
                Err(e) => {
                    log.line(&format!("Cannot read the SudoVDA monitors: {e}"));
                    w.read_failed(now);
                }
            },
            Due::Run(job) => {
                let ran = run(job, log);
                if let Some(msg) = w.done(ran, Instant::now()) {
                    log.line(&msg);
                }
            }
            Due::Wait => {}
        }
    }
}

/// Writes one line to `watch.log` (for the tray icon).
pub fn log_note(text: &str) {
    Log::open().line(text);
}

/// Restores the displays for the tray icon, as `urakata restore` does.
fn restore_now(log: &mut Log) -> Result<String, String> {
    let result = display::restore(false).and_then(|outcome| {
        if outcome.partial {
            Err(outcome.text)
        } else {
            Ok(outcome.text)
        }
    });
    match &result {
        Ok(text) => log.line(&format!("Restore from the tray icon:\n{text}")),
        Err(e) => log.line(&format!("Restore from the tray icon failed: {e}")),
    }
    result
}

/// Runs one job and logs what it did. The guard logs only a change.
fn run(job: Job, log: &mut Log) -> Ran {
    let result = match job {
        Job::Exclusive => display::exclusive(false, false),
        Job::Guard => match session_path() {
            Ok(file) if !file.exists() => return Ran::Stop,
            _ => display::exclusive(false, true),
        },
        Job::AfterStream => display::after_stream(),
    };
    match result {
        Ok(outcome) => {
            if job != Job::Guard || outcome.changed {
                log.line(&format!("{job:?}:\n{}", outcome.text));
            }
            // A partial restore kept the saved file: try again.
            if job == Job::AfterStream && outcome.partial {
                Ran::Failed
            } else if outcome.changed {
                Ran::Changed
            } else {
                Ran::Unchanged
            }
        }
        Err(e) => {
            log.line(&format!("{job:?} failed: {e}"));
            Ran::Failed
        }
    }
}

fn run_key() -> Result<windows_registry::Key, String> {
    windows_registry::CURRENT_USER
        .create(RUN_KEY)
        .map_err(|e| format!("cannot open HKCU\\{RUN_KEY}: {e}"))
}

/// Starts the watcher at each logon of this user (`Run` key, no admin).
pub fn install() -> Result<String, String> {
    let watch_exe = watch_exe()?;
    let command = autostart_command(&watch_exe.display().to_string());
    run_key()?
        .set_string(RUN_VALUE, &command)
        .map_err(|e| format!("cannot write the Run value: {e}"))?;
    Ok(format!(
        "The watcher starts at each logon:\n  HKCU\\{RUN_KEY}\\{RUN_VALUE} = {command}\n\
         Start it now with `urakata-watch`, or log off and on. Check that\n\
         Settings > Apps > Startup lists it as on."
    ))
}

/// Removes the logon start. A running watcher keeps running.
pub fn uninstall() -> Result<String, String> {
    let key = run_key()?;
    if key.get_type(RUN_VALUE).is_err() {
        return Ok("The watcher was not set to start at logon.".into());
    }
    key.remove_value(RUN_VALUE)
        .map_err(|e| format!("cannot remove the Run value: {e}"))?;
    Ok("The watcher no longer starts at logon. A running watcher keeps running.".into())
}
