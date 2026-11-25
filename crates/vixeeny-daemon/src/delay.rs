// SPDX-License-Identifier: GPL-3.0-or-later
//! The delay the user set before a capture: the press waits, then the capture runs as if pressed then. Pressing the
//! same capture again while it waits calls it off; another capture takes its place.

use vixeeny_common::ipc::ActionId;

/// What a press of a capture does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// It runs now.
    Now,
    /// It runs when the countdown `id` ends.
    Wait(u64),
    /// It called off the capture that was waiting.
    CalledOff,
}

#[derive(Debug, Default)]
pub struct Delays {
    last: u64,
    waiting: Option<(u64, ActionId)>,
}

impl Delays {
    /// A press of `action`, set to wait `seconds` (`0`: none).
    pub fn press(&mut self, action: ActionId, seconds: u8) -> Press {
        match self.waiting.take() {
            Some((_, waiting)) if waiting == action => Press::CalledOff,
            _ if seconds == 0 => Press::Now,
            _ => {
                self.last += 1;
                self.waiting = Some((self.last, action));
                Press::Wait(self.last)
            }
        }
    }

    /// The countdown `id` ended: the capture to run, unless it was called off or replaced.
    pub fn ended(&mut self, id: u64) -> Option<ActionId> {
        match self.waiting {
            Some((waiting, action)) if waiting == id => {
                self.waiting = None;
                Some(action)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_delay_the_capture_runs_now() {
        let mut d = Delays::default();
        assert_eq!(d.press(ActionId::CaptureRegion, 0), Press::Now);
    }

    #[test]
    fn a_delayed_capture_runs_once_its_countdown_ends() {
        let mut d = Delays::default();
        let Press::Wait(id) = d.press(ActionId::CaptureFullscreen, 3) else { panic!("not delayed") };
        assert_eq!(d.ended(id), Some(ActionId::CaptureFullscreen));
        // Once.
        assert_eq!(d.ended(id), None);
    }

    #[test]
    fn pressing_it_again_calls_it_off() {
        let mut d = Delays::default();
        let Press::Wait(id) = d.press(ActionId::CaptureRegion, 5) else { panic!("not delayed") };
        assert_eq!(d.press(ActionId::CaptureRegion, 5), Press::CalledOff);
        assert_eq!(d.ended(id), None);
        // The next press starts over.
        assert!(matches!(d.press(ActionId::CaptureRegion, 5), Press::Wait(_)));
    }

    #[test]
    fn another_capture_takes_its_place() {
        let mut d = Delays::default();
        let Press::Wait(first) = d.press(ActionId::CaptureRegion, 5) else { panic!("not delayed") };
        let Press::Wait(second) = d.press(ActionId::CaptureWindow, 3) else { panic!("not delayed") };
        assert_eq!(d.ended(first), None);
        assert_eq!(d.ended(second), Some(ActionId::CaptureWindow));
        // One without a delay runs now and drops the one waiting.
        let Press::Wait(third) = d.press(ActionId::CaptureRegion, 5) else { panic!("not delayed") };
        assert_eq!(d.press(ActionId::CaptureFullscreen, 0), Press::Now);
        assert_eq!(d.ended(third), None);
    }
}
