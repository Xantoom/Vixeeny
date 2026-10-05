// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget (plan 5.9) as a process of its own: `vixeeny-app --widget …`. The
//! recorder starts it and talks to it over its standard input and output (see `widget_math`), so
//! the app's action loop stays free while the pill is on screen. The widget window is excluded
//! from every screen capture (CA-REC-5) and never takes the focus.

#![cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use vixeeny_common::config::RecordingWidget;
use vixeeny_common::i18n::{Key, tr};
use vixeeny_platform::MonitorInfo;

use crate::widget_math::{FromWidget, ToWidget, corner_geometry};

/// The recorder's end: owns the child process.
pub struct Widget {
    child: Child,
    stdin: ChildStdin,
}

impl Widget {
    /// Starts the widget in the configured corner of `monitor`. `on_press` is called from a
    /// thread of its own for each button press.
    pub fn spawn(
        settings: &RecordingWidget,
        language: &str,
        monitor: &MonitorInfo,
        on_press: impl Fn(FromWidget) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (x, y, w, h) = corner_geometry(
            &settings.corner,
            (
                monitor.rect.x,
                monitor.rect.y,
                monitor.rect.width,
                monitor.rect.height,
            ),
            monitor.dpi,
        );
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--widget")
            .args([x.to_string(), y.to_string(), w.to_string(), h.to_string()])
            .arg(if settings.auto_hide { "1" } else { "0" })
            .arg(language)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .context("cannot start the recording widget")?;
        let stdin = child.stdin.take().context("no widget stdin")?;
        let stdout = child.stdout.take().context("no widget stdout")?;
        std::thread::Builder::new()
            .name("widget-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(press) = FromWidget::parse(&line) {
                        on_press(press);
                    }
                }
            })?;
        Ok(Self { child, stdin })
    }

    pub fn state(&mut self, paused: bool, elapsed: Duration) {
        let msg = ToWidget::State {
            paused,
            elapsed_ms: elapsed.as_millis() as u64,
        };
        let _ = self.stdin.write_all(msg.to_line().as_bytes());
        let _ = self.stdin.flush();
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(ToWidget::Quit.to_line().as_bytes());
        let _ = self.stdin.flush();
        // It exits by itself; do not leave a pill on screen if it does not.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `--widget <x> <y> <width> <height> <auto_hide 0|1> <language>`: the widget process.
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    use vixeeny_ui::widget_panel::{WidgetEvent, WidgetPanel, WidgetTexts};

    let num = |i: usize| -> anyhow::Result<i64> {
        args.get(i)
            .and_then(|a| a.parse().ok())
            .with_context(|| format!("bad widget argument {i}"))
    };
    let (x, y, w, h) = (
        num(0)? as i32,
        num(1)? as i32,
        num(2)? as u32,
        num(3)? as u32,
    );
    let auto_hide = args.get(4).is_some_and(|a| a == "1");
    let lang = crate::lang(args.get(5).map_or("auto", String::as_str));

    let panel = WidgetPanel::new(
        &WidgetTexts {
            pause: tr(Key::RecWidgetPause, lang).into(),
            resume: tr(Key::RecWidgetResume, lang).into(),
            stop: tr(Key::RecWidgetStop, lang).into(),
        },
        auto_hide,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    panel.set_geometry(x, y, w, h);
    panel.on_event(|event| {
        let press = match event {
            WidgetEvent::TogglePause => FromWidget::TogglePause,
            WidgetEvent::Stop => FromWidget::Stop,
        };
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(press.to_line().as_bytes());
        let _ = out.flush();
    });
    // Out of the captures from its creation: excluded only once shown, its first frames (a black
    // block) would be in the video. The guard lives as long as the event loop that creates it.
    let _creation = vixeeny_platform::without_open_animation();
    vixeeny_platform::exclude_new_windows(true);
    panel.show().map_err(|e| anyhow::anyhow!("{e}"))?;
    // Never in the video, never stealing the keyboard. If the exclusion fails the widget could
    // end up in the recording: that is logged loudly and the recording goes on.
    // The native window exists only once the event loop runs: the handle is fetched then (and
    // asked again for a moment if it is not there yet).
    vixeeny_ui::theme::when_native(panel.window(), |hwnd| {
        let id = vixeeny_platform::WindowId(hwnd);
        if let Err(e) = vixeeny_platform::exclude_from_capture(id) {
            tracing::error!("the widget cannot be excluded from the capture: {e}");
        }
        if let Err(e) = vixeeny_platform::set_noactivate_tool_window(id) {
            tracing::warn!("widget window style: {e}");
        }
    });

    // Messages from the recorder arrive on a thread and are applied on the event loop; when the
    // recorder is gone (or says quit) the widget closes.
    let handle = panel.handle();
    std::thread::Builder::new()
        .name("widget-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                match ToWidget::parse(&line) {
                    Some(ToWidget::State { paused, elapsed_ms }) => {
                        handle.set_state(paused, Duration::from_millis(elapsed_ms));
                    }
                    Some(ToWidget::Quit) => break,
                    None => {}
                }
            }
            handle.quit();
        })?;
    panel.run().map_err(|e| anyhow::anyhow!("{e}"))
}
