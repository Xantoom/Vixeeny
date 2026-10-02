// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget window (plan 5.9).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, SharedString};

use crate::RecordingWidget;

/// Translated accessible labels of the buttons.
#[derive(Debug, Clone, Default)]
pub struct WidgetTexts {
    pub pause: String,
    pub resume: String,
    pub stop: String,
}

/// What the buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetEvent {
    TogglePause,
    Stop,
}

/// `HH:MM:SS`.
pub fn format_elapsed(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// The time shown: it counts by itself between two updates and freezes while paused.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    base: Duration,
    since: Instant,
    paused: bool,
}

impl Clock {
    pub fn new(now: Instant) -> Self {
        Self {
            base: Duration::ZERO,
            since: now,
            paused: false,
        }
    }

    pub fn set(&mut self, paused: bool, elapsed: Duration, now: Instant) {
        self.base = elapsed;
        self.since = now;
        self.paused = paused;
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        if self.paused {
            self.base
        } else {
            self.base + now.saturating_duration_since(self.since)
        }
    }
}

thread_local! {
    /// The clock of the widget of this (event loop) thread.
    static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
}

/// A thread-safe way to drive a running widget.
#[derive(Clone)]
pub struct WidgetHandle(slint::Weak<RecordingWidget>);

impl WidgetHandle {
    pub fn set_state(&self, paused: bool, elapsed: Duration) {
        let _ = self.0.upgrade_in_event_loop(move |w| {
            let now = Instant::now();
            CLOCK.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.set(paused, elapsed, now);
                }
            });
            w.set_paused(paused);
            w.set_time(format_elapsed(elapsed).into());
        });
    }

    /// Ends the event loop (and so `WidgetPanel::run`).
    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    }
}

pub struct WidgetPanel {
    window: RecordingWidget,
    _ticker: slint::Timer,
}

impl WidgetPanel {
    pub fn new(texts: &WidgetTexts, auto_hide: bool) -> Result<Self, slint::PlatformError> {
        let window = RecordingWidget::new()?;
        window.set_pause_label(SharedString::from(texts.pause.as_str()));
        window.set_resume_label(SharedString::from(texts.resume.as_str()));
        window.set_stop_label(SharedString::from(texts.stop.as_str()));
        window.set_auto_hide(auto_hide);
        CLOCK.with(|c| *c.borrow_mut() = Some(Clock::new(Instant::now())));
        let ticker = slint::Timer::default();
        let weak = window.as_weak();
        ticker.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(200),
            move || {
                if let Some(w) = weak.upgrade() {
                    let elapsed = CLOCK.with(|c| c.borrow().map(|c| c.elapsed(Instant::now())));
                    if let Some(elapsed) = elapsed {
                        w.set_time(format_elapsed(elapsed).into());
                    }
                }
            },
        );
        Ok(Self {
            window,
            _ticker: ticker,
        })
    }

    pub fn handle(&self) -> WidgetHandle {
        WidgetHandle(self.window.as_weak())
    }

    pub fn window(&self) -> &RecordingWidget {
        &self.window
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn set_state(&self, paused: bool, elapsed: Duration) {
        self.window.set_paused(paused);
        self.window.set_time(format_elapsed(elapsed).into());
    }

    /// Physical pixels.
    pub fn set_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        let w = self.window.window();
        w.set_size(slint::PhysicalSize::new(width, height));
        w.set_position(slint::PhysicalPosition::new(x, y));
    }

    pub fn on_event(&self, on_event: impl Fn(WidgetEvent) + 'static) {
        let on_event = std::rc::Rc::new(on_event);
        let f = on_event.clone();
        self.window
            .on_pause_clicked(move || f(WidgetEvent::TogglePause));
        self.window
            .on_stop_clicked(move || on_event(WidgetEvent::Stop));
    }

    /// The native window handle (`HWND` on Windows) once the window is shown; the backend must
    /// be the winit one.
    #[cfg(feature = "desktop")]
    pub fn native_handle(&self) -> Option<u64> {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        self.window
            .window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get() as u64),
                _ => None,
            })
            .flatten()
    }

    /// Shows the window and runs the event loop until it is hidden or the app quits.
    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_counts_until_paused_then_freezes() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut c = Clock::new(t0);
        c.set(false, s(10), t0);
        assert_eq!(c.elapsed(t0 + s(3)), s(13));
        c.set(true, s(13), t0 + s(3));
        assert_eq!(c.elapsed(t0 + s(60)), s(13));
        c.set(false, s(13), t0 + s(60));
        assert_eq!(c.elapsed(t0 + s(62)), s(15));
    }

    #[test]
    fn elapsed_is_hh_mm_ss() {
        assert_eq!(format_elapsed(Duration::ZERO), "00:00:00");
        assert_eq!(format_elapsed(Duration::from_millis(59_999)), "00:00:59");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "01:02:05");
        assert_eq!(format_elapsed(Duration::from_secs(100 * 3600)), "100:00:00");
    }
}
