// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure parts of the recording widget: where it goes and how the two processes talk.
//! (The widget is a process of its own, `vixeeny-app --widget …`, so the action loop of the app
//! keeps answering the daemon while it is on screen.)
use vixeeny_common::i18n::{Key, Lang, size_label, tr};
/// The window's size at 96 DPI, in pixels: the bar and room around it for its shadow.
pub use vixeeny_overlay::widget::{Alert, HEIGHT, WIDTH};
/// Gap from the window to the screen edge at 96 DPI (the bar is 16 px from it).
pub const MARGIN: u32 = 6;

/// Where the widget's window can be on a monitor's work area (`x, y, width, height`, physical
/// pixels; `dpi` 96 = 100 %): its left-most and right-most x, its top-most and lowest y, and
/// its size.
fn room(work: (i32, i32, u32, u32), dpi: u32) -> ((i32, i32), (i32, i32), (u32, u32)) {
    let scale = |v: u32| (u64::from(v) * u64::from(dpi.max(48)) / 96) as u32;
    let (w, h, m) = (scale(WIDTH), scale(HEIGHT), scale(MARGIN));
    let (mx, my, mw, mh) = work;
    let left = mx + m as i32;
    let right = (mx + mw as i32 - w as i32 - m as i32).max(left);
    let top = my + m as i32;
    let bottom = (my + mh as i32 - h as i32 - m as i32).max(top);
    ((left, right), (top, bottom), (w, h))
}

/// Top-left corner and size, in physical pixels, of the widget at `corner` of a monitor's work
/// area (`custom`: at `custom`, fractions of the room across and down, see
/// [`vixeeny_common::config::RecordingWidget::custom`]).
pub fn corner_geometry(
    corner: &str,
    custom: (f32, f32),
    work: (i32, i32, u32, u32),
    dpi: u32,
) -> (i32, i32, u32, u32) {
    let ((left, right), (top, bottom), (w, h)) = room(work, dpi);
    let along =
        |lo: i32, hi: i32, f: f32| lo + ((hi - lo) as f32 * f.clamp(0.0, 1.0)).round() as i32;
    let (fx, fy) = match corner {
        "top_center" => (0.5, 0.0),
        "top_right" => (1.0, 0.0),
        "bottom_left" => (0.0, 1.0),
        "bottom_center" => (0.5, 1.0),
        "bottom_right" => (1.0, 1.0),
        "custom" => custom,
        _ => (0.0, 0.0),
    };
    (along(left, right, fx), along(top, bottom, fy), w, h)
}

/// The custom place of a widget whose window is at `(x, y)` on the monitor of `work`: the
/// inverse of [`corner_geometry`], to the thousandth.
pub fn custom_place(at: (i32, i32), work: (i32, i32, u32, u32), dpi: u32) -> (f32, f32) {
    let ((left, right), (top, bottom), _) = room(work, dpi);
    let part = |v: i32, lo: i32, hi: i32| {
        if hi <= lo {
            return 0.0;
        }
        let f = ((v - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0);
        (f * 1000.0).round() / 1000.0
    };
    (part(at.0, left, right), part(at.1, top, bottom))
}

/// A message from the recorder to the widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToWidget {
    /// State and the recorded time so far (pauses excluded).
    State {
        paused: bool,
        elapsed_ms: u64,
    },
    /// What goes wrong, and the words of its tooltip (one problem a line).
    Alert(Alert, String),
    Quit,
}

impl ToWidget {
    pub fn to_line(&self) -> String {
        match self {
            Self::State { paused, elapsed_ms } => {
                format!(
                    "state {} {elapsed_ms}\n",
                    if *paused { "paused" } else { "recording" }
                )
            }
            Self::Alert(level, text) => {
                let level = match level {
                    Alert::None => "none",
                    Alert::Warning => "warning",
                    Alert::Critical => "critical",
                };
                let text = text.replace('\\', "\\\\").replace('\n', "\\n");
                format!("alert {level} {text}\n")
            }
            Self::Quit => "quit\n".into(),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = line.strip_prefix("alert ") {
            let (level, text) = rest.split_once(' ').unwrap_or((rest, ""));
            let level = match level {
                "none" => Alert::None,
                "warning" => Alert::Warning,
                "critical" => Alert::Critical,
                _ => return None,
            };
            let mut out = String::with_capacity(text.len());
            let mut chars = text.chars();
            while let Some(c) = chars.next() {
                match (c, c == '\\') {
                    (_, true) => match chars.next()? {
                        'n' => out.push('\n'),
                        other => out.push(other),
                    },
                    (c, false) => out.push(c),
                }
            }
            return Some(Self::Alert(level, out));
        }
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "quit" => Some(Self::Quit),
            "state" => {
                let paused = match parts.next()? {
                    "paused" => true,
                    "recording" => false,
                    _ => return None,
                };
                Some(Self::State {
                    paused,
                    elapsed_ms: parts.next()?.parse().ok()?,
                })
            }
            _ => None,
        }
    }
}

/// Free space under which the disk alert turns red: the recording is about to fail.
pub const DISK_CRITICAL: u64 = 1 << 30;

/// How the recording goes, as the widget tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Health {
    /// Frames that never reached the file (the encoder or the GPU could not keep up).
    pub lost_frames: u64,
    /// `(free, total)` bytes of the disk the file is written to.
    pub disk: Option<(u64, u64)>,
}

impl Health {
    /// The disk has less than `disk_percent` % free (never with 0).
    pub fn disk_low(&self, disk_percent: u8) -> bool {
        self.disk.is_some_and(|(free, total)| {
            disk_percent > 0
                && u128::from(free) * 100 < u128::from(total) * u128::from(disk_percent)
        })
    }

    /// The alert for this health, with a disk alert under `disk_percent` % free (0: none).
    pub fn alert(&self, disk_percent: u8, lang: Lang) -> (Alert, String) {
        let mut level = Alert::None;
        let mut lines = Vec::new();
        if self.lost_frames > 0 {
            level = Alert::Warning;
            lines.push(
                tr(Key::WidgetFramesLost, lang).replace("{n}", &self.lost_frames.to_string()),
            );
        }
        if let Some((free, _)) = self.disk
            && self.disk_low(disk_percent)
        {
            level = if free < DISK_CRITICAL {
                Alert::Critical
            } else {
                level.max(Alert::Warning)
            };
            lines.push(tr(Key::WidgetDiskLow, lang).replace("{size}", &size_label(free, lang)));
        }
        (level, lines.join("\n"))
    }
}

/// A button press, from the widget to the recorder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FromWidget {
    TogglePause,
    Stop,
    /// The user moved the widget: its new custom place (see [`custom_place`]).
    Moved(f32, f32),
}

impl FromWidget {
    pub fn to_line(self) -> String {
        match self {
            Self::TogglePause => "pause\n".into(),
            Self::Stop => "stop\n".into(),
            Self::Moved(x, y) => format!("moved {x} {y}\n"),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        let press = match parts.next()? {
            "pause" => Self::TogglePause,
            "stop" => Self::Stop,
            "moved" => {
                let mut f = || {
                    parts
                        .next()?
                        .parse::<f32>()
                        .ok()
                        .filter(|f| (0.0..=1.0).contains(f))
                };
                Self::Moved(f()?, f()?)
            }
            _ => return None,
        };
        parts.next().is_none().then_some(press)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_hug_the_edges_with_a_margin_scaled_by_dpi() {
        let monitor = (1920, 0, 2560, 1440); // a second monitor to the right
        assert_eq!(
            corner_geometry("top_left", (0.0, 0.0), monitor, 96),
            (1926, 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("top_right", (0.0, 0.0), monitor, 96),
            (1920 + 2560 - 216 - 6, 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_left", (0.0, 0.0), monitor, 96),
            (1926, 1440 - 60 - 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_right", (0.0, 0.0), monitor, 96),
            (1920 + 2560 - 222, 1440 - 66, 216, 60)
        );
        // 200 % scaling doubles everything; an unknown corner is the default one.
        assert_eq!(
            corner_geometry("???", (0.0, 0.0), (0, 0, 3840, 2160), 192),
            (12, 12, 432, 120)
        );
    }

    #[test]
    fn the_middles_and_a_custom_place_share_the_room_left() {
        let work = (0, 0, 1920, 1032); // above a 48-pixel taskbar
        // 1920 - 216 - 2 × 6 = 1692 pixels of room across, 1032 - 60 - 12 = 960 down.
        assert_eq!(
            corner_geometry("top_center", (0.0, 0.0), work, 96),
            (852, 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_center", (0.0, 0.0), work, 96),
            (852, 966, 216, 60)
        );
        assert_eq!(
            corner_geometry("custom", (0.25, 0.5), work, 96),
            (429, 486, 216, 60)
        );
        // A place out of range stays on the screen.
        assert_eq!(
            corner_geometry("custom", (3.0, -1.0), work, 96),
            (1698, 6, 216, 60)
        );
        // Dropped somewhere: the same place comes back, on another screen too.
        let place = custom_place((429, 486), work, 96);
        assert_eq!(place, (0.25, 0.5));
        let big = (1920, 0, 3840, 2112);
        let (x, y, ..) = corner_geometry("custom", place, big, 192);
        assert_eq!(custom_place((x, y), big, 192), place);
        assert_eq!(custom_place((-500, 99_999), work, 96), (0.0, 1.0));
    }

    #[test]
    fn messages_round_trip() {
        for m in [
            ToWidget::Alert(Alert::Warning, "a \\ b\nc".into()),
            ToWidget::Alert(Alert::None, String::new()),
            ToWidget::State {
                paused: false,
                elapsed_ms: 0,
            },
            ToWidget::State {
                paused: true,
                elapsed_ms: 3_725_000,
            },
            ToWidget::Quit,
        ] {
            assert_eq!(ToWidget::parse(&m.to_line()), Some(m.clone()));
        }
        for b in [
            FromWidget::TogglePause,
            FromWidget::Stop,
            FromWidget::Moved(0.25, 1.0),
        ] {
            assert_eq!(FromWidget::parse(&b.to_line()), Some(b));
        }
        for bad in ["moved", "moved 0.5", "moved 2 0", "moved a b", "stop now"] {
            assert_eq!(FromWidget::parse(bad), None, "{bad}");
        }
        for bad in [
            "",
            "state",
            "state running 5",
            "state paused x",
            "dance",
            "alert loud x",
        ] {
            assert_eq!(ToWidget::parse(bad), None, "{bad}");
        }
        assert_eq!(FromWidget::parse("go"), None);
    }

    #[test]
    fn lost_frames_and_a_full_disk_raise_the_alert() {
        let en = Lang::En;
        const GB: u64 = 1 << 30;
        assert_eq!(Health::default().alert(5, en), (Alert::None, String::new()));
        let lost = Health {
            lost_frames: 42,
            disk: Some((500 * GB, 1000 * GB)),
        };
        let (level, text) = lost.alert(5, en);
        assert_eq!(level, Alert::Warning);
        assert!(text.starts_with("42 frame"), "{text}");
        // Under 5 % free: a warning; under 1 GB: red.
        let low = Health {
            lost_frames: 0,
            disk: Some((40 * GB, 1000 * GB)),
        };
        assert_eq!(low.alert(5, en).0, Alert::Warning);
        assert_eq!(low.alert(2, en).0, Alert::None);
        assert_eq!(low.alert(0, en).0, Alert::None);
        let full = Health {
            lost_frames: 3,
            disk: Some((GB / 2, 1000 * GB)),
        };
        let (level, text) = full.alert(5, Lang::Fr);
        assert_eq!(level, Alert::Critical);
        assert_eq!(text.lines().count(), 2);
        assert!(text.ends_with("512 Mo libres"), "{text}");
    }
}
