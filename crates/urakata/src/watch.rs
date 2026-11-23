//! Timing rules of `urakata watch` that need no Windows API. The loop
//! that calls Windows is in `watcher.rs`.
//!
//! The watcher keeps the set of present SudoVDA monitors. A new monitor
//! means a stream started: run `exclusive`. No monitor left means the
//! stream ended: check the layout and delete the saved file (Windows
//! turns the physical displays on again by itself).
//!
//! The start delay, the guard time and the guard limit come from
//! `Settings`. Steam sets its stream layout (physical displays on again,
//! HDR on the virtual one) about 4 s after the monitor arrives (measured
//! on a test host), so `exclusive` waits and a guard checks that the
//! physical displays stay off. The guard turns them off again only a
//! limited number of times, so that it never fights another program in a
//! loop.
//!
//! After sleep, Windows can turn the physical displays on again while the
//! SudoVDA monitor stays, so no monitor event comes. A resume
//! during a stream starts the guard again.

#![cfg_attr(not(windows), allow(dead_code))]

use std::time::{Duration, Instant};

use crate::settings::Settings;

/// Wait after a device event before the monitor list is read, so that a
/// burst of events gives one read.
pub const SETTLE: Duration = Duration::from_secs(1);
/// Read the monitor list at least this often, in case an event is lost.
pub const POLL: Duration = Duration::from_secs(60);
/// Wait between `exclusive` tries.
pub const RETRY: Duration = Duration::from_secs(1);
/// `exclusive` tries per new stream.
pub const EXCLUSIVE_TRIES: u32 = 15;
/// Wait between guard checks.
pub const GUARD_STEP: Duration = Duration::from_secs(1);
/// Wait after the last SudoVDA monitor went away. Windows turns the
/// physical displays on within 2 s.
pub const END_DELAY: Duration = Duration::from_secs(3);
/// Wait between tries of the after-stream check.
pub const END_RETRY: Duration = Duration::from_secs(5);
/// Tries of the after-stream check.
pub const END_TRIES: u32 = 3;

/// Work that the watcher does at a set time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// A stream started: make the SudoVDA displays the only active ones.
    Exclusive,
    /// Shortly after `exclusive`: while the saved layout file exists, run
    /// `exclusive` again if a physical display is on again.
    Guard,
    /// No stream (it ended, or the watcher started without one): delete
    /// the saved layout, or restore it if Windows did not.
    AfterStream,
}

/// Result of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// The job failed.
    Failed,
    /// The job ran and changed nothing.
    Unchanged,
    /// The job changed the display layout.
    Changed,
    /// Guard only: no saved layout (for example after `restore`), stop.
    Stop,
}

/// What the loop must do now.
#[derive(Debug, PartialEq, Eq)]
pub enum Due {
    /// Read the SudoVDA monitor list and pass it to `Watch::seen`.
    Read,
    /// Run the job and pass the result to `Watch::done`.
    Run(Job),
    /// Nothing yet; wait until `Watch::deadline`.
    Wait,
}

#[derive(Debug)]
struct Planned {
    job: Job,
    at: Instant,
    /// Failed tries (Exclusive, AfterStream) or re-applied layouts (Guard).
    count: u32,
    /// Guard only: end of the guard time.
    until: Instant,
}

impl Planned {
    fn new(job: Job, at: Instant) -> Self {
        Self {
            job,
            at,
            count: 0,
            until: at,
        }
    }
}

/// State of the watcher.
#[derive(Debug)]
pub struct Watch {
    known: Vec<String>,
    read_at: Instant,
    planned: Option<Planned>,
    settings: Settings,
    /// The PC resumed; the next monitor read decides about the guard.
    resumed: bool,
}

impl Watch {
    /// Starts with the monitors present now. Without a stream, the
    /// after-stream check runs at once: this repairs a layout that a crash
    /// or a logoff during a stream left behind.
    pub fn new(present: Vec<String>, now: Instant) -> Self {
        let planned = present
            .is_empty()
            .then(|| Planned::new(Job::AfterStream, now));
        Self {
            known: normalized(present),
            read_at: now + POLL,
            planned,
            settings: Settings::default(),
            resumed: false,
        }
    }

    /// SudoVDA monitors were present at the last read.
    pub fn streaming(&self) -> bool {
        !self.known.is_empty()
    }

    /// Settings for the jobs planned from now on.
    /// A planned guard ends when the new settings turn it off.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings.clamped();
        if !self.guard_on() && self.planned.as_ref().is_some_and(|p| p.job == Job::Guard) {
            self.planned = None;
        }
    }

    fn guard_on(&self) -> bool {
        let s = &self.settings;
        s.exclusive && s.guard_s > 0 && s.guard_reapply > 0
    }

    /// Plans the guard from `now` for the guard time. A running guard
    /// keeps its count of re-applied layouts, so that two resume events
    /// of one wake do not double the limit. Returns false when the guard
    /// is off.
    fn start_guard(&mut self, now: Instant) -> bool {
        if !self.guard_on() {
            return false;
        }
        let until = now + self.settings.guard();
        match &mut self.planned {
            Some(p) if p.job == Job::Guard => p.until = p.until.max(until),
            _ => {
                self.planned = Some(Planned {
                    until,
                    ..Planned::new(Job::Guard, now + GUARD_STEP)
                })
            }
        }
        true
    }

    /// A monitor arrived or went away.
    pub fn event(&mut self, now: Instant) {
        // Keep an earlier, sooner read: a stream of events must not delay
        // it forever.
        self.read_at = self.read_at.min(now + SETTLE);
    }

    /// The next time `due` has work.
    pub fn deadline(&self) -> Instant {
        match &self.planned {
            Some(p) => p.at.min(self.read_at),
            None => self.read_at,
        }
    }

    /// The PC resumed from sleep. Reads the monitor list soon, because
    /// events can be lost during sleep. That read also reads the settings
    /// and, during a stream, starts the guard again: Windows can turn the
    /// physical displays on at wake while the SudoVDA monitor stays.
    pub fn resumed(&mut self, now: Instant) {
        self.event(now);
        self.resumed = true;
    }

    /// The work for now. A read comes first, because it can cancel a job.
    pub fn due(&self, now: Instant) -> Due {
        if self.read_at <= now {
            Due::Read
        } else {
            match &self.planned {
                Some(p) if p.at <= now => Due::Run(p.job),
                _ => Due::Wait,
            }
        }
    }

    /// The monitor list could not be read. Try again later.
    pub fn read_failed(&mut self, now: Instant) {
        self.read_at = now + END_RETRY;
    }

    /// The SudoVDA monitors present now. Returns a line for the log when
    /// a stream started or ended.
    pub fn seen(&mut self, present: Vec<String>, now: Instant) -> Option<String> {
        self.read_at = now + POLL;
        let present = normalized(present);
        let new = present.iter().any(|id| !self.known.contains(id));
        let ended = present.is_empty() && !self.known.is_empty();
        let resumed = std::mem::take(&mut self.resumed);
        self.known = present;
        // A job that no longer fits the state is dropped.
        let stream_job = self.planned.as_ref().map(|p| p.job != Job::AfterStream);
        if stream_job == Some(self.known.is_empty()) {
            self.planned = None;
        }
        if new {
            let started = format!("Stream started: {}", self.known.join(", "));
            if !self.settings.exclusive {
                self.planned = None;
                return Some(format!("{started}. Exclusive is off in the settings."));
            }
            let at = now + self.settings.start_delay();
            self.planned = Some(Planned::new(Job::Exclusive, at));
            Some(started)
        } else if ended {
            self.planned = Some(Planned::new(Job::AfterStream, now + END_DELAY));
            Some("Stream ended.".into())
        } else if resumed
            && !self.known.is_empty()
            // A planned `exclusive` starts its own guard after it ran.
            && self.planned.as_ref().is_none_or(|p| p.job != Job::Exclusive)
            && self.start_guard(now)
        {
            Some("Resumed from sleep during a stream. Guarding the layout.".into())
        } else {
            None
        }
    }

    /// The job ran. Plans what follows: a retry after a failure (up to a
    /// limit), the guard after `exclusive`. Returns a line for the log
    /// when the watcher gives up or the guard acts.
    pub fn done(&mut self, ran: Ran, now: Instant) -> Option<String> {
        let mut p = self.planned.take()?;
        match (p.job, ran) {
            (Job::Exclusive | Job::AfterStream, Ran::Failed) => {
                p.count += 1;
                let (limit, wait) = match p.job {
                    Job::Exclusive => (EXCLUSIVE_TRIES, RETRY),
                    _ => (END_TRIES, END_RETRY),
                };
                if p.count >= limit {
                    return Some(format!("Giving up on {:?} after {} tries.", p.job, p.count));
                }
                p.at = now + wait;
                self.planned = Some(p);
                None
            }
            (Job::Exclusive, _) => {
                self.start_guard(now);
                None
            }
            (Job::Guard, Ran::Stop) => None,
            (Job::Guard, _) => {
                let mut msg = None;
                if ran == Ran::Changed {
                    p.count += 1;
                    msg = Some(format!(
                        "Another program turned displays on again ({} of {}).",
                        p.count, self.settings.guard_reapply
                    ));
                }
                p.at = now + GUARD_STEP;
                if p.count < self.settings.guard_reapply && p.at <= p.until {
                    self.planned = Some(p);
                }
                msg
            }
            (Job::AfterStream, _) => None,
        }
    }
}

/// Instance IDs compare without case.
fn normalized(mut ids: Vec<String>) -> Vec<String> {
    for id in &mut ids {
        id.make_ascii_uppercase();
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Start command for the `Run` registry value: `urakata-watch.exe`, the
/// watcher without a console window. Windows does not start or list a
/// `Run` value that starts `conhost.exe` (seen in a logon test).
pub fn autostart_command(watch_exe: &str) -> String {
    format!(r#""{watch_exe}""#)
}

#[cfg(test)]
mod tests {
    use super::*;

    const START_DELAY: Duration = Duration::from_secs(3);
    const GUARD: Duration = Duration::from_secs(20);
    const GUARD_REAPPLY: u32 = 3;

    #[test]
    fn defaults_match_the_tested_timing() {
        let s = Settings::default();
        assert_eq!(s.start_delay(), START_DELAY);
        assert_eq!(s.guard(), GUARD);
        assert_eq!(s.guard_reapply, GUARD_REAPPLY);
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn start_without_stream_checks_at_once() {
        let t = Instant::now();
        let w = Watch::new(vec![], t);
        assert_eq!(w.due(t), Due::Run(Job::AfterStream));
    }

    #[test]
    fn start_during_stream_leaves_it() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        assert_eq!(w.due(t), Due::Wait);
        assert_eq!(w.deadline(), t + POLL);
        // Same monitor on the next read: no stream start.
        assert_eq!(w.seen(ids(&["v1"]), t + POLL), None);
        assert_eq!(w.due(t + POLL), Due::Wait);
    }

    #[test]
    fn events_settle_into_one_read() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.event(t);
        w.event(t + Duration::from_millis(500));
        assert_eq!(w.deadline(), t + SETTLE);
        assert_eq!(w.due(t + Duration::from_millis(900)), Due::Wait);
        assert_eq!(w.due(t + SETTLE), Due::Read);
    }

    #[test]
    fn new_monitor_runs_exclusive_with_retries() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&[]), t);
        assert_eq!(w.done(Ran::Unchanged, t), None);
        assert!(w.seen(ids(&["V1"]), t).is_some());
        assert_eq!(w.due(t), Due::Wait);
        let mut now = t + START_DELAY;
        assert_eq!(w.due(now), Due::Run(Job::Exclusive));
        for _ in 1..EXCLUSIVE_TRIES {
            assert_eq!(w.done(Ran::Failed, now), None);
            assert_eq!(w.due(now), Due::Wait);
            now += RETRY;
            assert_eq!(w.due(now), Due::Run(Job::Exclusive));
        }
        assert!(w.done(Ran::Failed, now).is_some());
        assert_eq!(w.due(now + RETRY), Due::Wait);
    }

    /// Runs the guard after `exclusive` at `t`; `steam` says when another
    /// program turns the displays on again. Returns the guard runs.
    fn guard_runs(steam: impl Fn(u32) -> bool) -> u32 {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V2"]), t);
        let mut now = t + START_DELAY;
        assert_eq!(w.due(now), Due::Run(Job::Exclusive));
        w.done(Ran::Changed, now);
        let mut runs = 0;
        loop {
            now += GUARD_STEP;
            match w.due(now) {
                Due::Run(Job::Guard) => {}
                _ => return runs,
            }
            runs += 1;
            let ran = if steam(runs) {
                Ran::Changed
            } else {
                Ran::Unchanged
            };
            w.done(ran, now);
        }
    }

    #[test]
    fn streaming_follows_the_last_read() {
        let t = Instant::now();
        let mut w = Watch::new(Vec::new(), t);
        assert!(!w.streaming());
        w.seen(ids(&["V1"]), t);
        assert!(w.streaming());
        w.seen(Vec::new(), t);
        assert!(!w.streaming());
        assert!(Watch::new(ids(&["V1"]), t).streaming());
    }

    #[test]
    fn guard_ends_after_its_time() {
        assert_eq!(guard_runs(|_| false), 20);
    }

    #[test]
    fn guard_reapplies_a_limited_number_of_times() {
        assert_eq!(guard_runs(|n| n == 3), 20);
        assert_eq!(guard_runs(|_| true), GUARD_REAPPLY);
    }

    #[test]
    fn guard_stops_without_saved_layout() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V2"]), t);
        w.done(Ran::Unchanged, t);
        assert_eq!(w.due(t + GUARD_STEP), Due::Run(Job::Guard));
        assert_eq!(w.done(Ran::Stop, t + GUARD_STEP), None);
        assert_eq!(w.due(t + GUARD), Due::Wait);
    }

    #[test]
    fn stream_end_checks_after_delay() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        assert_eq!(w.seen(vec![], t).as_deref(), Some("Stream ended."));
        assert_eq!(w.due(t), Due::Wait);
        assert_eq!(w.deadline(), t + END_DELAY);
        assert_eq!(w.due(t + END_DELAY), Due::Run(Job::AfterStream));
        assert_eq!(w.done(Ran::Unchanged, t + END_DELAY), None);
        assert_eq!(w.due(t + END_DELAY), Due::Wait);
    }

    #[test]
    fn new_stream_cancels_end_check() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(vec![], t);
        assert!(w.seen(ids(&["V2"]), t + SETTLE).is_some());
        assert_eq!(w.due(t + SETTLE + START_DELAY), Due::Run(Job::Exclusive));
    }

    #[test]
    fn stream_end_cancels_exclusive_and_guard() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V1", "V2"]), t);
        w.seen(vec![], t);
        assert_eq!(w.due(t + START_DELAY), Due::Run(Job::AfterStream));

        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V2"]), t);
        w.done(Ran::Changed, t);
        w.seen(vec![], t);
        assert_eq!(w.due(t + GUARD_STEP), Due::Wait);
        assert_eq!(w.due(t + END_DELAY), Due::Run(Job::AfterStream));
    }

    #[test]
    fn wake_swaps_monitors_without_end() {
        // After wake the old monitor goes away when the new one comes.
        let t = Instant::now();
        let mut w = Watch::new(ids(&["UID258"]), t);
        assert!(w.seen(ids(&["UID259"]), t).is_some());
        assert_eq!(w.due(t + START_DELAY), Due::Run(Job::Exclusive));
    }

    #[test]
    fn exclusive_off_only_logs_the_start() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.set_settings(Settings {
            exclusive: false,
            ..Settings::default()
        });
        let msg = w.seen(ids(&["V2"]), t).unwrap();
        assert!(msg.ends_with("Exclusive is off in the settings."));
        assert_eq!(w.deadline(), t + POLL);
        // The end check still runs.
        w.seen(vec![], t);
        assert_eq!(w.due(t + END_DELAY), Due::Run(Job::AfterStream));
    }

    #[test]
    fn settings_change_start_delay_and_guard() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.set_settings(Settings {
            start_delay_s: 10,
            guard_s: 0,
            ..Settings::default()
        });
        w.seen(ids(&["V2"]), t);
        assert_eq!(w.due(t + START_DELAY), Due::Wait);
        let at = t + Duration::from_secs(10);
        assert_eq!(w.due(at), Due::Run(Job::Exclusive));
        // No guard with a guard time of 0.
        assert_eq!(w.done(Ran::Changed, at), None);
        assert_eq!(w.deadline(), t + POLL);

        // Nor with 0 guard repeats.
        let mut w = Watch::new(ids(&["V1"]), t);
        w.set_settings(Settings {
            guard_reapply: 0,
            ..Settings::default()
        });
        w.seen(ids(&["V2"]), t);
        assert_eq!(w.done(Ran::Changed, t + START_DELAY), None);
        assert_eq!(w.deadline(), t + POLL);
    }

    #[test]
    fn guard_limit_from_settings() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.set_settings(Settings {
            guard_reapply: 1,
            ..Settings::default()
        });
        w.seen(ids(&["V2"]), t);
        w.done(Ran::Changed, t);
        assert_eq!(w.due(t + GUARD_STEP), Due::Run(Job::Guard));
        let msg = w.done(Ran::Changed, t + GUARD_STEP).unwrap();
        assert!(msg.ends_with("(1 of 1)."));
        assert_eq!(w.due(t + GUARD_STEP * 2), Due::Wait);
    }

    const RESUMED: &str = "Resumed from sleep during a stream. Guarding the layout.";

    #[test]
    fn resume_during_stream_guards_again() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V2"]), t);
        w.done(Ran::Changed, t);
        // The guard ended long ago; the PC sleeps and wakes.
        let wake = t + Duration::from_secs(3600);
        assert_eq!(w.due(wake), Due::Read);
        assert_eq!(w.seen(ids(&["V2"]), wake), None);
        w.resumed(wake);
        // The monitor list is read first; that read starts the guard.
        assert_eq!(w.due(wake + SETTLE), Due::Read);
        let read = wake + SETTLE;
        assert_eq!(w.seen(ids(&["V2"]), read).as_deref(), Some(RESUMED));
        assert_eq!(w.due(read + GUARD_STEP), Due::Run(Job::Guard));
        let msg = w.done(Ran::Changed, read + GUARD_STEP).unwrap();
        assert!(msg.ends_with("(1 of 3)."));
        // It ends after the guard time.
        let mut now = read + GUARD_STEP;
        let mut runs = 1;
        loop {
            now += GUARD_STEP;
            if w.due(now) != Due::Run(Job::Guard) {
                break;
            }
            runs += 1;
            w.done(Ran::Unchanged, now);
        }
        assert_eq!(runs, 20);
        // Only one read starts a guard per resume.
        assert_eq!(w.seen(ids(&["V2"]), now), None);
    }

    #[test]
    fn second_resume_event_keeps_the_guard_count() {
        // Windows sends two resume events when the user wakes the PC.
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.resumed(t);
        assert!(w.seen(ids(&["V1"]), t + SETTLE).is_some());
        let mut now = t + SETTLE;
        for _ in 0..2 {
            now += GUARD_STEP;
            assert_eq!(w.due(now), Due::Run(Job::Guard));
            w.done(Ran::Changed, now);
        }
        w.resumed(now);
        assert!(w.seen(ids(&["V1"]), now + SETTLE).is_some());
        now += SETTLE + GUARD_STEP;
        assert_eq!(w.due(now), Due::Run(Job::Guard));
        let msg = w.done(Ran::Changed, now).unwrap();
        assert!(msg.ends_with("(3 of 3)."));
        assert_eq!(w.due(now + GUARD_STEP), Due::Wait);
    }

    #[test]
    fn resume_guard_stops_without_saved_layout() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.resumed(t);
        w.seen(ids(&["V1"]), t + SETTLE);
        let at = t + SETTLE + GUARD_STEP;
        assert_eq!(w.due(at), Due::Run(Job::Guard));
        assert_eq!(w.done(Ran::Stop, at), None);
        assert_eq!(w.deadline(), t + SETTLE + POLL);
    }

    #[test]
    fn resume_without_stream_only_reads() {
        let t = Instant::now();
        let mut w = Watch::new(vec![], t);
        w.done(Ran::Unchanged, t);
        w.resumed(t);
        assert_eq!(w.deadline(), t + SETTLE);
        assert_eq!(w.due(t + SETTLE), Due::Read);
        assert_eq!(w.seen(vec![], t + SETTLE), None);
        assert_eq!(w.deadline(), t + SETTLE + POLL);
    }

    #[test]
    fn resume_keeps_a_planned_exclusive() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.seen(ids(&["V2"]), t);
        w.resumed(t);
        assert_eq!(w.seen(ids(&["V2"]), t + SETTLE), None);
        assert_eq!(w.due(t + START_DELAY), Due::Run(Job::Exclusive));
    }

    #[test]
    fn resume_with_new_monitor_runs_exclusive() {
        // A real stream gets a new monitor at wake.
        let t = Instant::now();
        let mut w = Watch::new(ids(&["UID258"]), t);
        w.resumed(t);
        let msg = w.seen(ids(&["UID259"]), t + SETTLE).unwrap();
        assert!(msg.starts_with("Stream started"));
        assert_eq!(w.due(t + SETTLE + GUARD_STEP), Due::Wait);
        assert_eq!(w.due(t + SETTLE + START_DELAY), Due::Run(Job::Exclusive));
    }

    #[test]
    fn resume_that_ends_the_stream_checks_after_it() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.resumed(t);
        assert_eq!(w.seen(vec![], t + SETTLE).as_deref(), Some("Stream ended."));
        assert_eq!(w.due(t + SETTLE + GUARD_STEP), Due::Wait);
        assert_eq!(w.due(t + SETTLE + END_DELAY), Due::Run(Job::AfterStream));
    }

    fn guard_off_settings() -> [Settings; 3] {
        [
            Settings {
                exclusive: false,
                ..Settings::default()
            },
            Settings {
                guard_s: 0,
                ..Settings::default()
            },
            Settings {
                guard_reapply: 0,
                ..Settings::default()
            },
        ]
    }

    #[test]
    fn resume_uses_the_settings_of_the_read_after_it() {
        let t = Instant::now();
        // Turned off before sleep, read after wake: no guard.
        for settings in guard_off_settings() {
            let mut w = Watch::new(ids(&["V1"]), t);
            w.resumed(t);
            w.set_settings(settings);
            assert_eq!(w.seen(ids(&["V1"]), t + SETTLE), None);
            assert_eq!(w.deadline(), t + SETTLE + POLL);
        }
        // Turned on before sleep: the guard starts.
        let mut w = Watch::new(ids(&["V1"]), t);
        w.set_settings(Settings {
            exclusive: false,
            ..Settings::default()
        });
        w.resumed(t);
        w.set_settings(Settings::default());
        assert!(w.seen(ids(&["V1"]), t + SETTLE).is_some());
    }

    #[test]
    fn settings_that_turn_the_guard_off_end_it() {
        let t = Instant::now();
        for settings in guard_off_settings() {
            let mut w = Watch::new(ids(&["V1"]), t);
            w.seen(ids(&["V2"]), t);
            w.done(Ran::Changed, t);
            assert_eq!(w.due(t + GUARD_STEP), Due::Run(Job::Guard));
            w.set_settings(settings);
            assert_eq!(w.due(t + GUARD_STEP), Due::Wait);
        }
    }

    #[test]
    fn failed_read_retries() {
        let t = Instant::now();
        let mut w = Watch::new(ids(&["V1"]), t);
        w.event(t);
        w.read_failed(t + SETTLE);
        assert_eq!(w.deadline(), t + SETTLE + END_RETRY);
    }

    #[test]
    fn autostart_quotes_paths() {
        assert_eq!(
            autostart_command(r"C:\Tools\urakata-watch.exe"),
            r#""C:\Tools\urakata-watch.exe""#
        );
    }
}
