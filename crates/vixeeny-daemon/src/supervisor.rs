// SPDX-License-Identifier: GPL-3.0-or-later
//! Starts `vixeeny-app` and reports its exit. The daemon never polls the child: a helper thread blocks in `wait()`.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use vixeeny_common::ipc::{ActionId, Frozen};

use crate::core::{Event, SpawnId};
use crate::server::EventTx;

pub trait Spawner {
    /// Starts the app so that it runs `action` (on the `frozen` screens, if any); its exit must later be reported as
    /// [`Event::AppExited`] with the same `id`.
    fn spawn(&mut self, id: SpawnId, action: ActionId, frozen: Option<&Frozen>, tx: EventTx) -> io::Result<()>;

    /// Shows the app's notification card `kind` (see its `--toast`) with `text`.
    fn show_card(&mut self, _kind: &str, _text: &str) {}
}

/// Launches the `vixeeny-app` executable installed next to the daemon.
pub struct ProcessSpawner {
    app_path: PathBuf,
}

impl ProcessSpawner {
    pub fn next_to_current_exe() -> io::Result<Self> {
        let mut path = std::env::current_exe()?;
        path.set_file_name(format!("vixeeny-app{}", std::env::consts::EXE_SUFFIX));
        Ok(Self { app_path: path })
    }

    pub fn new(app_path: PathBuf) -> Self {
        Self { app_path }
    }
}

/// Detects the hardware encoders in the background (`vixeeny-app --warm-probe`), as soon as the daemon starts, so the
/// settings and the first recording find the answer in the cache.
pub fn warm_probe() {
    let Ok(spawner) = ProcessSpawner::next_to_current_exe() else {
        return;
    };
    let mut command = Command::new(&spawner.app_path);
    command.arg("--warm-probe");
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::debug!("cannot start the hardware detection: {e}"),
    }
}

impl Spawner for ProcessSpawner {
    fn spawn(&mut self, id: SpawnId, action: ActionId, frozen: Option<&Frozen>, tx: EventTx) -> io::Result<()> {
        let mut command = Command::new(&self.app_path);
        command.arg("--action").arg(action.cli_name());
        if let Some(frozen) = frozen {
            command.arg("--frozen").arg(frozen.to_arg());
        }
        let mut child = command.spawn()?;
        tracing::debug!("started {} (pid {}, launch {id})", self.app_path.display(), child.id());
        std::thread::Builder::new().name(format!("app-wait-{id}")).stack_size(64 * 1024).spawn(move || {
            let status = child.wait();
            let code = status.as_ref().ok().and_then(std::process::ExitStatus::code);
            // What a crash was (the app's own errors are in its log).
            if let Some(code) = code.filter(|c| *c != 0).map(|c| c as u32) {
                match vixeeny_platform::crash::describe_exit(code) {
                    Some(what) => {
                        tracing::error!("the app crashed: exception 0x{code:08x} ({what})");
                    }
                    None => tracing::warn!("the app exited with code {code}"),
                }
            }
            let success = status.is_ok_and(|status| status.success());
            tx.send(Event::AppExited { id, success });
        })?;
        Ok(())
    }

    fn show_card(&mut self, kind: &str, text: &str) {
        let card = Command::new(&self.app_path).arg("--toast").args([kind, text]).spawn();
        if let Err(e) = card {
            tracing::warn!("cannot show the card: {e}");
        }
    }
}
