// SPDX-License-Identifier: GPL-3.0-or-later
//! The side overlay (plan 5.12): a strip of actions on one edge of the screen.
//!
//! The pure parts (the entries, the keyboard order, where the window goes) are plain functions;
//! [`SidePanel`] puts them on a Slint window.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, VecModel};

use crate::{SideItem, SidePanelWindow};

/// Which edge of the screen the strip sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// `left`, `right`, `top`, `bottom`; anything else is the default, the right edge.
    pub fn from_setting(setting: &str) -> Self {
        match setting {
            "left" => Self::Left,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => Self::Right,
        }
    }

    /// A column (left or right edge) rather than a row.
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    const fn index(self) -> i32 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Top => 2,
            Self::Bottom => 3,
        }
    }
}

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Region,
    Window,
    Screen,
    AllMonitors,
    Scrolling,
    RecordToggle,
    ReplayToggle,
    ReplaySave,
    Settings,
}

const HEADER_ID: i32 = 200;

impl Choice {
    const ALL: [Self; 9] = [
        Self::Region,
        Self::Window,
        Self::Screen,
        Self::AllMonitors,
        Self::Scrolling,
        Self::RecordToggle,
        Self::ReplayToggle,
        Self::ReplaySave,
        Self::Settings,
    ];

    const fn id(self) -> i32 {
        self as i32
    }

    fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Translated texts.
#[derive(Debug, Clone, Default)]
pub struct SideTexts {
    pub image: String,
    pub region: String,
    pub window: String,
    pub screen: String,
    pub all_monitors: String,
    pub scrolling: String,
    pub video: String,
    pub record: String,
    pub stop_recording: String,
    pub replay_start: String,
    pub replay_stop: String,
    pub replay_save: String,
    pub settings: String,
    /// The buttons of the strip's corner.
    pub pin: String,
    pub close: String,
}

/// What the strip needs to know when it opens.
#[derive(Debug, Clone)]
pub struct SideState {
    pub recording: bool,
    pub replay: bool,
    pub dark: bool,
    pub edge: Edge,
    /// `false` when the OS asks for fewer animations: the strip just appears.
    pub animate: bool,
    /// The OS blurs the background behind the window.
    pub backdrop: bool,
    /// Opens pinned: it stays until closed (it was pinned the last time).
    pub pinned: bool,
}

/// One row of the strip, before it becomes a Slint item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i32,
    pub glyph: i32,
    pub label: String,
    pub enabled: bool,
    pub header: bool,
    pub active: bool,
}

impl Entry {
    fn action(choice: Choice, glyph: i32, label: &str) -> Self {
        Self {
            id: choice.id(),
            glyph,
            label: label.to_owned(),
            enabled: true,
            header: false,
            active: false,
        }
    }

    fn header(n: i32, label: &str) -> Self {
        Self {
            id: HEADER_ID + n,
            glyph: 0,
            label: label.to_uppercase(),
            enabled: false,
            header: true,
            active: false,
        }
    }

    const fn selectable(&self) -> bool {
        self.enabled && !self.header
    }
}

/// The entries, in display order.
pub fn entries(texts: &SideTexts, state: &SideState) -> Vec<Entry> {
    let mut list = vec![
        Entry::header(0, &texts.image),
        Entry::action(Choice::Region, 0, &texts.region),
        Entry::action(Choice::Window, 1, &texts.window),
        Entry::action(Choice::Screen, 2, &texts.screen),
        Entry::action(Choice::AllMonitors, 3, &texts.all_monitors),
        Entry::action(Choice::Scrolling, 4, &texts.scrolling),
        Entry::header(1, &texts.video),
    ];
    let mut record = if state.recording {
        Entry::action(Choice::RecordToggle, 7, &texts.stop_recording)
    } else {
        Entry::action(Choice::RecordToggle, 6, &texts.record)
    };
    record.active = state.recording;
    list.push(record);
    let mut replay = Entry::action(
        Choice::ReplayToggle,
        8,
        if state.replay {
            &texts.replay_stop
        } else {
            &texts.replay_start
        },
    );
    replay.active = state.replay;
    list.push(replay);
    let mut save = Entry::action(Choice::ReplaySave, 9, &texts.replay_save);
    save.enabled = state.replay;
    list.push(save);
    list.push(Entry::action(Choice::Settings, 11, &texts.settings));
    list
}

/// The entry reached by moving `delta` (±1) selectable entries from `current`, wrapping around.
/// `current` is returned when nothing can be selected.
pub fn next_selectable(entries: &[Entry], current: i32, delta: i32) -> i32 {
    let n = entries.len() as i32;
    if n == 0 {
        return current;
    }
    let mut at = current;
    for _ in 0..n {
        at = (at + delta).rem_euclid(n);
        if entries[at as usize].selectable() {
            return at;
        }
    }
    current
}

/// The first entry the keyboard can reach.
pub fn first_selectable(entries: &[Entry]) -> i32 {
    entries
        .iter()
        .position(Entry::selectable)
        .map_or(-1, |i| i as i32)
}

// Layout constants, in logical pixels, matching `Side` in `side_panel.slint`.
const PAD: f64 = 6.0;
const GAP: f64 = 2.0;
const MARGIN: f64 = 12.0;
const BUTTON: f64 = 40.0;
const SEPARATOR: f64 = 9.0;
const SMALL: f64 = 28.0;
/// Beside the strip, for the name of the action under the pointer.
const TIP_ROOM: f64 = 220.0;
const TIP_HEIGHT: f64 = 40.0;

/// The window's size in logical pixels: the strip (the pin and close buttons, a separator, the
/// entries), the room for the label beside it, and the margin it slides in through.
fn logical_size(entries: &[Entry], vertical: bool) -> (f64, f64) {
    let along: f64 = entries
        .iter()
        .map(|e| if e.header { SEPARATOR } else { BUTTON })
        .sum();
    let along = 2.0 * PAD + SMALL + SEPARATOR + along + GAP * (entries.len() + 1) as f64;
    let across = BUTTON + 2.0 * PAD;
    if vertical {
        (across + TIP_ROOM + 2.0 * MARGIN, along + 2.0 * MARGIN)
    } else {
        (along + 2.0 * MARGIN, across + TIP_HEIGHT + 2.0 * MARGIN)
    }
}

/// Where the window goes on `monitor` (`x, y, width, height`, physical pixels; `dpi` 96 = 100 %):
/// flush against `edge`, centred along it. Returns `x, y, width, height` in physical pixels.
pub fn panel_geometry(
    edge: Edge,
    monitor: (i32, i32, u32, u32),
    dpi: u32,
    entries: &[Entry],
) -> (i32, i32, u32, u32) {
    let scale = f64::from(dpi.max(48)) / 96.0;
    let (mx, my, mw, mh) = monitor;
    let (w, h) = logical_size(entries, edge.is_vertical());
    let w = ((w * scale).round() as u32).min(mw);
    let h = ((h * scale).round() as u32).min(mh);
    let centred_x = mx + (mw - w) as i32 / 2;
    let centred_y = my + (mh - h) as i32 / 2;
    let (x, y) = match edge {
        Edge::Left => (mx, centred_y),
        Edge::Right => (mx + (mw - w) as i32, centred_y),
        Edge::Top => (centred_x, my),
        Edge::Bottom => (centred_x, my + (mh - h) as i32),
    };
    (x, y, w, h)
}

/// Whether the dark theme applies: the `general.theme` setting, or the OS's choice for `system`.
pub fn dark_theme(setting: &str, os_prefers_dark: bool) -> bool {
    match setting {
        "dark" => true,
        "light" => false,
        _ => os_prefers_dark,
    }
}

const ENTER: Duration = Duration::from_millis(220);
const EXIT: Duration = Duration::from_millis(150);

fn to_item(entry: &Entry) -> SideItem {
    SideItem {
        id: entry.id,
        glyph: entry.glyph,
        label: entry.label.as_str().into(),
        enabled: entry.enabled,
        header: entry.header,
        active: entry.active,
    }
}

pub struct SidePanel {
    window: SidePanelWindow,
    entries: Rc<Vec<Entry>>,
    chosen: Rc<Cell<Option<Choice>>>,
    animate: bool,
}

impl SidePanel {
    pub fn new(texts: &SideTexts, state: &SideState) -> Result<Self, slint::PlatformError> {
        let window = SidePanelWindow::new()?;
        let entries = Rc::new(entries(texts, state));
        let model = Rc::new(VecModel::from(
            entries.iter().map(to_item).collect::<Vec<_>>(),
        ));
        window.set_items(model.clone().into());
        window.set_vertical(state.edge.is_vertical());
        window.set_edge(state.edge.index());
        crate::theme::apply(
            &window,
            crate::theme::Look::new(state.dark, None, state.animate),
        );
        window.set_backdrop(state.backdrop);
        let (enter, exit) = if state.animate {
            (ENTER, EXIT)
        } else {
            (Duration::ZERO, Duration::ZERO)
        };
        window.set_enter(enter.as_millis() as i64);
        window.set_exit(exit.as_millis() as i64);
        window.set_selected(first_selectable(&entries));
        window.set_pinned(state.pinned);
        window.set_pin_label(texts.pin.as_str().into());
        window.set_close_label(texts.close.as_str().into());

        let chosen = Rc::new(Cell::new(None));
        let panel = Self {
            window,
            entries: entries.clone(),
            chosen: chosen.clone(),
            animate: state.animate,
        };

        let list = entries.clone();
        panel.window.on_navigate({
            let weak = panel.window.as_weak();
            move |delta| {
                let current = weak.upgrade().map_or(-1, |w| w.get_selected());
                next_selectable(&list, current, delta)
            }
        });

        let weak = panel.window.as_weak();
        let closing = panel.closer();
        panel.window.on_activate(move |id| {
            let Some(choice) = Choice::from_id(id) else {
                return;
            };
            // A disabled entry (a replay save without a replay) does nothing.
            if let Some(w) = weak.upgrade()
                && w.get_items().iter().any(|row| row.id == id && !row.enabled)
            {
                return;
            }
            chosen.set(Some(choice));
            closing();
        });
        let closing = panel.closer();
        panel.window.on_dismiss(closing);
        Ok(panel)
    }

    /// Starts the exit animation, then ends the event loop.
    fn closer(&self) -> impl Fn() + 'static {
        let weak = self.window.as_weak();
        let exit = if self.animate { EXIT } else { Duration::ZERO };
        let started = Rc::new(Cell::new(false));
        move || {
            if started.replace(true) {
                return;
            }
            if let Some(w) = weak.upgrade() {
                w.set_shown(false);
            }
            // A little longer than the animation, so its last frame is drawn.
            slint::Timer::single_shot(exit + Duration::from_millis(30), || {
                let _ = slint::quit_event_loop();
            });
        }
    }

    pub fn window(&self) -> &SidePanelWindow {
        &self.window
    }

    /// The entries, as shown.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What was picked, if anything.
    pub fn chosen(&self) -> Option<Choice> {
        self.chosen.get()
    }

    /// The user pinned the strip: it comes back after the action it was closed for.
    pub fn pinned(&self) -> bool {
        self.window.get_pinned()
    }

    /// Physical pixels.
    pub fn set_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        let w = self.window.window();
        w.set_size(slint::PhysicalSize::new(width, height));
        w.set_position(slint::PhysicalPosition::new(x, y));
    }

    /// The native window handle (`HWND`) once the window is shown.
    #[cfg(feature = "desktop")]
    pub fn native_handle(&self) -> Option<u64> {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        self.window
            .window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get() as u64),
                // The content `NSView`, which the platform layer dresses.
                RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as u64),
                _ => None,
            })
            .flatten()
    }

    /// Shows the strip, slides it in, and runs until something is picked, Escape is pressed, or
    /// the window loses the focus (a click elsewhere). Once the animation is over nothing runs:
    /// the event loop sleeps until the next input.
    pub fn run(&self) -> Result<Option<Choice>, slint::PlatformError> {
        self.run_with(|_| false)
    }

    /// Like [`run`](Self::run); once the window exists `dress` gets its native handle and says
    /// whether the OS now blurs the background behind it (a lighter fill then lets it show).
    pub fn run_with(
        &self,
        dress: impl FnOnce(u64) -> bool,
    ) -> Result<Option<Choice>, slint::PlatformError> {
        self.window.show()?;
        #[cfg(feature = "desktop")]
        {
            use slint::winit_030::winit::event::WindowEvent;
            use slint::winit_030::{EventResult, WinitWindowAccessor};
            if let Some(handle) = self.native_handle() {
                self.window.set_backdrop(dress(handle));
            }
            let closing = self.closer();
            let weak = self.window.as_weak();
            self.window.window().on_winit_window_event(move |_, event| {
                // The user went to another window: the strip goes, unless it is pinned.
                let pinned = weak.upgrade().is_some_and(|w| w.get_pinned());
                if matches!(event, WindowEvent::Focused(false)) && !pinned {
                    closing();
                }
                EventResult::Propagate
            });
        }
        #[cfg(not(feature = "desktop"))]
        let _ = dress;
        // The slide starts once the window is on screen: started at `show`, it would be over
        // before the first frame (creating the window takes longer than the animation).
        let weak = self.window.as_weak();
        let once = Rc::new(Cell::new(false));
        let notified = self
            .window
            .window()
            .set_rendering_notifier(move |state, _| {
                if matches!(state, slint::RenderingState::AfterRendering) && !once.replace(true) {
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(w) = weak.upgrade() {
                            w.set_shown(true);
                        }
                    });
                }
            });
        if notified.is_err() {
            self.window.set_shown(true);
        }
        slint::run_event_loop()?;
        self.window.hide()?;
        Ok(self.chosen.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> SideTexts {
        SideTexts {
            image: "Screenshot".into(),
            region: "Region".into(),
            window: "Window".into(),
            screen: "Screen".into(),
            all_monitors: "All screens".into(),
            scrolling: "Scrolling capture".into(),
            video: "Video".into(),
            record: "Record".into(),
            stop_recording: "Stop recording".into(),
            replay_start: "Start replay buffer".into(),
            replay_stop: "Stop replay buffer".into(),
            replay_save: "Save replay".into(),
            settings: "Settings".into(),
            pin: "Keep open".into(),
            close: "Close".into(),
        }
    }

    pub(crate) fn state() -> SideState {
        SideState {
            recording: false,
            replay: false,
            dark: true,
            edge: Edge::Right,
            animate: false,
            backdrop: false,
            pinned: false,
        }
    }

    #[test]
    fn the_theme_follows_the_setting_or_the_os() {
        assert!(dark_theme("dark", false));
        assert!(!dark_theme("light", true));
        assert!(dark_theme("system", true));
        assert!(!dark_theme("system", false));
        assert!(dark_theme("whatever", true));
    }

    #[test]
    fn edges_parse_with_the_right_edge_as_default() {
        assert_eq!(Edge::from_setting("left"), Edge::Left);
        assert_eq!(Edge::from_setting("top"), Edge::Top);
        assert_eq!(Edge::from_setting("bottom"), Edge::Bottom);
        assert_eq!(Edge::from_setting("right"), Edge::Right);
        assert_eq!(Edge::from_setting("diagonal"), Edge::Right);
        assert!(Edge::Left.is_vertical() && !Edge::Bottom.is_vertical());
    }

    #[test]
    fn entries_follow_the_state() {
        let mut s = state();
        let list = entries(&texts(), &s);
        let find =
            |list: &[Entry], c: Choice| list.iter().find(|e| e.id == c.id()).unwrap().clone();
        assert_eq!(find(&list, Choice::RecordToggle).label, "Record");
        assert!(
            !find(&list, Choice::ReplaySave).enabled,
            "no replay, nothing to save"
        );
        s.recording = true;
        s.replay = true;
        let list = entries(&texts(), &s);
        let record = find(&list, Choice::RecordToggle);
        assert_eq!(
            (record.label.as_str(), record.active),
            ("Stop recording", true)
        );
        assert_eq!(
            find(&list, Choice::ReplayToggle).label,
            "Stop replay buffer"
        );
        assert!(find(&list, Choice::ReplaySave).enabled);
    }

    #[test]
    fn the_keyboard_skips_titles_and_disabled_entries_and_wraps() {
        let list = entries(&texts(), &state());
        let first = first_selectable(&list);
        assert_eq!(list[first as usize].id, Choice::Region.id());
        // Down from the last entry wraps to the first; up from the first wraps to the last.
        let last = list.len() as i32 - 1;
        assert_eq!(next_selectable(&list, last, 1), first);
        assert_eq!(next_selectable(&list, first, -1), last);
        // Down from the scrolling capture jumps over the "Video" title to Record.
        let scrolling = list
            .iter()
            .position(|e| e.id == Choice::Scrolling.id())
            .unwrap() as i32;
        let record = list
            .iter()
            .position(|e| e.id == Choice::RecordToggle.id())
            .unwrap() as i32;
        assert_eq!(next_selectable(&list, scrolling, 1), record);
        // The replay save is disabled while there is no replay: Replay → Settings.
        let replay = list
            .iter()
            .position(|e| e.id == Choice::ReplayToggle.id())
            .unwrap() as i32;
        let settings = list
            .iter()
            .position(|e| e.id == Choice::Settings.id())
            .unwrap() as i32;
        assert_eq!(next_selectable(&list, replay, 1), settings);
        assert_eq!(next_selectable(&[], 0, 1), 0);
    }

    #[test]
    fn the_window_hugs_its_edge_and_is_centred_along_it() {
        let list = entries(&texts(), &state());
        let monitor = (0, 0, 2560, 1440);
        let (x, y, w, h) = panel_geometry(Edge::Right, monitor, 96, &list);
        assert_eq!(x + w as i32, 2560);
        assert_eq!(y, (1440 - h as i32) / 2);
        assert_eq!(panel_geometry(Edge::Left, monitor, 96, &list).0, 0);
        let (x, y, w, h) = panel_geometry(Edge::Bottom, monitor, 96, &list);
        assert_eq!(y + h as i32, 1440);
        assert_eq!(x, (2560 - w as i32) / 2);
        assert_eq!(panel_geometry(Edge::Top, monitor, 96, &list).1, 0);
        // A second monitor to the right, at 200 %: everything doubles and moves with it.
        let second = (2560, 0, 3840, 2160);
        let (x1, _, w1, h1) = panel_geometry(Edge::Right, second, 192, &list);
        let (_, _, w0, h0) = panel_geometry(Edge::Right, second, 96, &list);
        assert_eq!(x1 + w1 as i32, 2560 + 3840);
        assert_eq!((w1, h1), (w0 * 2, h0 * 2));
        // Never bigger than the monitor.
        let tiny = (0, 0, 300, 400);
        let (_, _, w, h) = panel_geometry(Edge::Right, tiny, 96, &list);
        assert!(w <= 300 && h <= 400);
    }
}
