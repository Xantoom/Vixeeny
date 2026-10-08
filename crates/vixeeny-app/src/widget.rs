// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget (plan 5.9) as a process of its own: `vixeeny-app --widget …`. The
//! recorder starts it and talks to it over its standard input and output (see `widget_math`), so
//! the app's action loop stays free while the pill is on screen. The widget window is excluded
//! from every screen capture (CA-REC-5) and never takes the focus.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use vixeeny_common::config::RecordingWidget;
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
            .arg(monitor.dpi.to_string())
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

/// `--widget <x> <y> <width> <height> <auto_hide 0|1> <dpi>`: the widget process.
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    use std::rc::Rc;
    use vixeeny_overlay::widget::{Widget as Bar, WidgetEvent};

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
    let dpi = num(5)? as u32;

    vixeeny_platform::ensure_dpi_aware();
    let look = vixeeny_overlay::Look {
        animations: vixeeny_platform::animations_enabled(),
        ..vixeeny_overlay::Look::default()
    };
    // Out of the captures from its creation, never stealing the keyboard.
    let bar = Rc::new(Bar::new((x, y, w, h), dpi, look, auto_hide)?);
    bar.on_event(|event| {
        let press = match event {
            WidgetEvent::TogglePause => FromWidget::TogglePause,
            WidgetEvent::Stop => FromWidget::Stop,
        };
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(press.to_line().as_bytes());
        let _ = out.flush();
    });

    // Messages from the recorder arrive on a thread and are applied on the widget's; when the
    // recorder is gone (or says quit) the widget closes.
    let (_mailbox, sender) = vixeeny_overlay::popup::mailbox({
        let bar = bar.clone();
        move |msg: ToWidget| match msg {
            ToWidget::State { paused, elapsed_ms } => {
                bar.set_state(paused, Duration::from_millis(elapsed_ms));
            }
            ToWidget::Quit => bar.close(),
        }
    })?;
    std::thread::Builder::new()
        .name("widget-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                match ToWidget::parse(&line) {
                    Some(ToWidget::Quit) => break,
                    Some(msg) => {
                        sender.send(msg);
                    }
                    None => {}
                }
            }
            sender.send(ToWidget::Quit);
        })?;
    bar.run()?;
    Ok(())
}
