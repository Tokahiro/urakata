//! Controller input: turns XInput button and stick states into window
//! actions. No Windows API here, so the tests run anywhere; `window.rs`
//! reads the controller and carries out the actions.

use std::time::Duration;

/// What a controller button does in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    /// A: press the focused control.
    Select,
    /// B or View: back to the sidebar, then close.
    Back,
    /// LB: the page before.
    PrevPage,
    /// RB: the next page.
    NextPage,
}

// XInput button bits (XINPUT_GAMEPAD_*).
pub const DPAD_UP: u16 = 0x0001;
pub const DPAD_DOWN: u16 = 0x0002;
pub const DPAD_LEFT: u16 = 0x0004;
pub const DPAD_RIGHT: u16 = 0x0008;
pub const VIEW: u16 = 0x0020;
pub const LB: u16 = 0x0100;
pub const RB: u16 = 0x0200;
pub const A: u16 = 0x1000;
pub const B: u16 = 0x2000;

const DIRECTIONS: [(u16, Action); 4] = [
    (DPAD_UP, Action::Up),
    (DPAD_DOWN, Action::Down),
    (DPAD_LEFT, Action::Left),
    (DPAD_RIGHT, Action::Right),
];
const DIRS: u16 = DPAD_UP | DPAD_DOWN | DPAD_LEFT | DPAD_RIGHT;
const BUTTONS: [(u16, Action); 5] = [
    (A, Action::Select),
    (B, Action::Back),
    (VIEW, Action::Back),
    (LB, Action::PrevPage),
    (RB, Action::NextPage),
];

/// The bits the window uses.
const USED: u16 = DIRS | VIEW | LB | RB | A | B;

/// The stick counts as a direction past this deflection (of 32767)...
const STICK_ON: i16 = 16_000;
/// ...and lets go below this one, so that it does not flicker at the edge.
const STICK_OFF: i16 = 12_000;
/// A held direction repeats after this time...
const REPEAT_FIRST: Duration = Duration::from_millis(400);
/// ...and then this often.
const REPEAT_NEXT: Duration = Duration::from_millis(120);

/// The state between two reads of one controller.
#[derive(Debug, Default)]
pub struct Pad {
    /// Buttons and stick directions of the last read; `None` before the
    /// first read, which only sets them.
    last: Option<u16>,
    /// Directions of the stick, as D-pad bits.
    stick: u16,
    /// The direction that repeats while held, and the time to its next step.
    held: Option<(u16, Duration)>,
    /// The direction of the last step is a repeat, not a new press.
    repeat: bool,
}

impl Pad {
    /// Forgets the state: the next read only sets it. For a controller that
    /// went away and for the time the window is not in front, so that a
    /// button held in Big Picture does nothing when the window comes back.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// One read: `buttons` are the XInput bits, `x` and `y` the left stick
    /// (up is positive), `dt` the time since the last read.
    pub fn step(&mut self, buttons: u16, x: i16, y: i16, dt: Duration) -> Vec<Action> {
        self.repeat = false;
        self.stick = stick_dirs(self.stick, x, y);
        let now = (buttons & USED) | self.stick;
        let Some(last) = self.last.replace(now) else {
            return Vec::new();
        };
        let pressed = now & !last;
        let mut out: Vec<Action> = BUTTONS
            .iter()
            .filter(|(bit, _)| pressed & bit != 0)
            .map(|&(_, a)| a)
            .collect();
        if let Some(&(bit, a)) = DIRECTIONS.iter().find(|(bit, _)| pressed & bit != 0) {
            out.push(a);
            self.held = Some((bit, REPEAT_FIRST));
        } else if let Some((bit, wait)) = self.held {
            if now & bit == 0 {
                self.held = None;
            } else if let Some(left) = wait.checked_sub(dt).filter(|l| !l.is_zero()) {
                self.held = Some((bit, left));
            } else {
                out.extend(DIRECTIONS.iter().filter(|d| d.0 == bit).map(|d| d.1));
                self.repeat = true;
                self.held = Some((bit, REPEAT_NEXT));
            }
        }
        out
    }
}

impl Pad {
    /// Whether the direction of the last step came from a held direction.
    pub fn repeated(&self) -> bool {
        self.repeat
    }
}

/// The stick as D-pad bits, with a gap between on and off per axis.
fn stick_dirs(last: u16, x: i16, y: i16) -> u16 {
    axis(last, x, DPAD_RIGHT, DPAD_LEFT) | axis(last, y, DPAD_UP, DPAD_DOWN)
}

fn axis(last: u16, v: i16, plus: u16, minus: u16) -> u16 {
    let v = i32::from(v);
    if v > i32::from(STICK_ON) {
        plus
    } else if v < -i32::from(STICK_ON) {
        minus
    } else if v.abs() < i32::from(STICK_OFF) {
        0
    } else {
        last & (plus | minus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(30);

    #[test]
    fn first_read_only_sets_the_state() {
        let mut p = Pad::default();
        // A is still down from the launch in Big Picture.
        assert!(p.step(A, 0, 0, T).is_empty());
        assert!(p.step(A, 0, 0, T).is_empty());
        assert!(p.step(0, 0, 0, T).is_empty());
        assert_eq!(p.step(A, 0, 0, T), vec![Action::Select]);
        p.reset();
        assert!(p.step(B, 0, 0, T).is_empty());
    }

    #[test]
    fn buttons_act_once_per_press() {
        let mut p = Pad::default();
        p.step(0, 0, 0, T);
        assert_eq!(p.step(B, 0, 0, T), vec![Action::Back]);
        assert!(p.step(B, 0, 0, T).is_empty());
        assert_eq!(p.step(VIEW, 0, 0, T), vec![Action::Back]);
        assert_eq!(
            p.step(LB | RB, 0, 0, T),
            vec![Action::PrevPage, Action::NextPage]
        );
        // Start, X and Y do nothing.
        assert!(p.step(0x0010 | 0x4000 | 0x8000, 0, 0, T).is_empty());
    }

    #[test]
    fn held_direction_repeats() {
        let mut p = Pad::default();
        p.step(0, 0, 0, T);
        assert_eq!(p.step(DPAD_DOWN, 0, 0, T), vec![Action::Down]);
        assert!(!p.repeated());
        // Nothing until 400 ms...
        let mut steps = 0;
        let mut got = Vec::new();
        while got.is_empty() {
            got = p.step(DPAD_DOWN, 0, 0, T);
            steps += 1;
        }
        assert_eq!(got, vec![Action::Down]);
        assert!(p.repeated());
        assert_eq!(steps, 14); // 14 x 30 ms >= 400 ms
        // ...then every 120 ms.
        let n = (0..4)
            .map(|_| p.step(DPAD_DOWN, 0, 0, T).len())
            .sum::<usize>();
        assert_eq!(n, 1);
        // Let go: no more steps.
        assert!(p.step(0, 0, 0, T).is_empty());
        assert!(p.step(0, 0, 0, Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn a_new_direction_wins() {
        let mut p = Pad::default();
        p.step(0, 0, 0, T);
        assert_eq!(p.step(DPAD_DOWN, 0, 0, T), vec![Action::Down]);
        assert_eq!(p.step(DPAD_DOWN | DPAD_RIGHT, 0, 0, T), vec![Action::Right]);
        assert!(p.step(DPAD_DOWN, 0, 0, T).is_empty());
    }

    #[test]
    fn stick_has_a_gap() {
        let mut p = Pad::default();
        p.step(0, 0, 0, T);
        assert!(p.step(0, 0, 15_000, T).is_empty());
        assert_eq!(p.step(0, 0, 20_000, T), vec![Action::Up]);
        // Between off and on: still up, so no new press.
        assert!(p.step(0, 0, 13_000, T).is_empty());
        assert!(p.step(0, 0, 20_000, T).is_empty());
        // Below off: let go; then down.
        assert!(p.step(0, 0, 0, T).is_empty());
        assert_eq!(p.step(0, 0, -32_768, T), vec![Action::Down]);
        assert!(p.step(0, 0, 0, T).is_empty());
        assert_eq!(p.step(0, 32_767, 0, T), vec![Action::Right]);
        assert!(p.step(0, 0, 0, T).is_empty());
        assert_eq!(p.step(0, -20_000, 0, T), vec![Action::Left]);
    }
}
