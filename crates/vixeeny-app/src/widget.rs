// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget as a process of its own: `vixeeny-app --widget …`. The recorder starts it and talks to it over
//! its standard input and output (see `widget_math`), so the app's action loop stays free while the pill is on screen.
//! The widget window is excluded from every screen capture and never takes the focus.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use vixeeny_common::config::RecordingWidget;
use vixeeny_platform::{MonitorInfo, PhysicalRect};

use crate::widget_math::{Alert, FromWidget, ToWidget, corner_fraction, corner_geometry};

/// The recorder's end: owns the child process.
pub struct Widget {
    child: Child,
    stdin: ChildStdin,
}

impl Widget {
    /// Starts the widget in the configured corner of `monitor`. `on_press` is called from a thread of its own for each
    /// button press.
    pub fn spawn(
        settings: &RecordingWidget,
        monitor: &MonitorInfo,
        on_press: impl Fn(FromWidget) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let PhysicalRect { x, y, width: w, height: h } =
            corner_geometry(&settings.corner, settings.custom, monitor.work, monitor.dpi);
        let (fx, fy) = corner_fraction(&settings.corner, settings.custom);
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--widget")
            .args([x.to_string(), y.to_string(), w.to_string(), h.to_string()])
            .arg(&settings.display)
            .arg(monitor.dpi.to_string())
            .args([fx.to_string(), fy.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().context("cannot start the recording widget")?;
        let stdin = child.stdin.take().context("no widget stdin")?;
        let stdout = child.stdout.take().context("no widget stdout")?;
        std::thread::Builder::new().name("widget-reader".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(press) = FromWidget::parse(&line) {
                    on_press(press);
                }
            }
        })?;
        Ok(Self { child, stdin })
    }

    pub fn state(&mut self, paused: bool, elapsed: Duration) {
        self.send(&ToWidget::State { paused, elapsed_ms: elapsed.as_millis() as u64 });
    }

    /// What goes wrong, and the words of the warning sign's tooltip.
    pub fn alert(&mut self, level: Alert, text: &str) {
        self.send(&ToWidget::Alert(level, text.to_owned()));
    }

    fn send(&mut self, msg: &ToWidget) {
        let _ = self.stdin.write_all(msg.to_line().as_bytes());
        let _ = self.stdin.flush();
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(ToWidget::Quit.to_line().as_bytes());
        let _ = self.stdin.flush();
        // It exits by itself (Windows tells when); do not leave a pill on screen if it does not within a second.
        let (tx, rx) = std::sync::mpsc::channel();
        if vixeeny_platform::on_process_exit(self.child.id(), move || {
            let _ = tx.send(());
        }) && rx.recv_timeout(Duration::from_secs(1)).is_ok()
        {
            let _ = self.child.wait();
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `--widget <x> <y> <width> <height> <always|fade|alerts> <dpi> <across> <down>`: the widget process (`across`,
/// `down`: where it is on the screen, 0 left or top to 1 right or bottom).
pub fn run_child(args: &[String]) -> anyhow::Result<()> {
    use std::rc::Rc;
    use vixeeny_overlay::widget::{Display, Widget as Bar, WidgetEvent};

    let num = |i: usize| -> anyhow::Result<i64> {
        args.get(i).and_then(|a| a.parse().ok()).with_context(|| format!("bad widget argument {i}"))
    };
    let geometry = PhysicalRect::new(num(0)? as i32, num(1)? as i32, num(2)? as u32, num(3)? as u32);
    let display = Display::from_name(args.get(4).map_or("", String::as_str));
    let dpi = num(5)? as u32;
    let fraction = |i: usize| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let anchor = (fraction(6), fraction(7));

    vixeeny_platform::ensure_dpi_aware();
    let config = vixeeny_common::paths::config_file()
        .and_then(|p| vixeeny_common::config::Config::load(&p).ok())
        .unwrap_or_default();
    let look = vixeeny_overlay::Look::new(
        crate::dark(&config),
        vixeeny_platform::system_accent(),
        vixeeny_platform::animations_enabled(),
    );
    // Out of the captures from its creation, never stealing the keyboard.
    let bar = Rc::new(Bar::new(geometry, anchor, dpi, look, display)?);
    bar.on_event(move |event| {
        let press = match event {
            WidgetEvent::TogglePause => FromWidget::TogglePause,
            WidgetEvent::Stop => FromWidget::Stop,
        };
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(press.to_line().as_bytes());
        let _ = out.flush();
    });

    // Messages from the recorder arrive on a thread and are applied on the widget's; when the recorder is gone (or says
    // quit) the widget closes.
    let (_mailbox, sender) = vixeeny_overlay::popup::mailbox({
        let bar = bar.clone();
        move |msg: ToWidget| match msg {
            ToWidget::State { paused, elapsed_ms } => {
                bar.set_state(paused, Duration::from_millis(elapsed_ms));
            }
            ToWidget::Alert(level, text) => bar.set_alert(level, &text),
            ToWidget::Quit => bar.close(),
        }
    })?;
    std::thread::Builder::new().name("widget-stdin".into()).spawn(move || {
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
