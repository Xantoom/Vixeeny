// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure parts of the recording widget: where it goes and how the two processes talk.
//! (The widget is a process of its own, `vixeeny-app --widget …`, so the action loop of the app
//! keeps answering the daemon while it is on screen.)
use vixeeny_common::i18n::{Key, Lang, size_label, tr};
/// The window's size at 96 DPI, in pixels: the bar and room around it for its shadow.
pub use vixeeny_overlay::widget::{Alert, HEIGHT, WIDTH};
/// Gap from the window to the screen edge at 96 DPI (the bar is 16 px from it).
pub const MARGIN: u32 = 6;

/// Top-left corner and size, in physical pixels, of the widget in `corner` of a monitor
/// (`x, y, width, height` of the monitor in physical pixels; `dpi` 96 = 100 %).
pub fn corner_geometry(
    corner: &str,
    monitor: (i32, i32, u32, u32),
    dpi: u32,
) -> (i32, i32, u32, u32) {
    let scale = |v: u32| (u64::from(v) * u64::from(dpi.max(48)) / 96) as u32;
    let (w, h, m) = (scale(WIDTH), scale(HEIGHT), scale(MARGIN));
    let (mx, my, mw, mh) = monitor;
    let left = mx + m as i32;
    let right = mx + mw as i32 - w as i32 - m as i32;
    let top = my + m as i32;
    let bottom = my + mh as i32 - h as i32 - m as i32;
    let (x, y) = match corner {
        "top_right" => (right, top),
        "bottom_left" => (left, bottom),
        "bottom_right" => (right, bottom),
        _ => (left, top),
    };
    (x, y, w, h)
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
        if let Some((free, total)) = self.disk
            && disk_percent > 0
            && u128::from(free) * 100 < u128::from(total) * u128::from(disk_percent)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FromWidget {
    TogglePause,
    Stop,
}

impl FromWidget {
    pub fn to_line(self) -> &'static str {
        match self {
            Self::TogglePause => "pause\n",
            Self::Stop => "stop\n",
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            "pause" => Some(Self::TogglePause),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_hug_the_edges_with_a_margin_scaled_by_dpi() {
        let monitor = (1920, 0, 2560, 1440); // a second monitor to the right
        assert_eq!(corner_geometry("top_left", monitor, 96), (1926, 6, 216, 60));
        assert_eq!(
            corner_geometry("top_right", monitor, 96),
            (1920 + 2560 - 216 - 6, 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_left", monitor, 96),
            (1926, 1440 - 60 - 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_right", monitor, 96),
            (1920 + 2560 - 222, 1440 - 66, 216, 60)
        );
        // 200 % scaling doubles everything; an unknown corner is the default one.
        assert_eq!(
            corner_geometry("???", (0, 0, 3840, 2160), 192),
            (12, 12, 432, 120)
        );
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
        for b in [FromWidget::TogglePause, FromWidget::Stop] {
            assert_eq!(FromWidget::parse(b.to_line()), Some(b));
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
