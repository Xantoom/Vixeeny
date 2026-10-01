// SPDX-License-Identifier: GPL-3.0-or-later
//! Starts `vixeeny-app` and reports its exit. The daemon never polls the child: a helper
//! thread blocks in `wait()`.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use vixeeny_common::ipc::ActionId;

use crate::core::{Event, SpawnId};
use crate::server::EventTx;

pub trait Spawner {
    /// Starts the app so that it runs `action`; its exit must later be reported as
    /// [`Event::AppExited`] with the same `id`.
    fn spawn(&mut self, id: SpawnId, action: ActionId, tx: EventTx) -> io::Result<()>;
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

impl Spawner for ProcessSpawner {
    fn spawn(&mut self, id: SpawnId, action: ActionId, tx: EventTx) -> io::Result<()> {
        let mut child = Command::new(&self.app_path)
            .arg("--action")
            .arg(action.cli_name())
            .spawn()?;
        tracing::info!(
            "started {} (pid {}, launch {id})",
            self.app_path.display(),
            child.id()
        );
        std::thread::Builder::new()
            .name(format!("app-wait-{id}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                let success = child.wait().is_ok_and(|status| status.success());
                tx.send(Event::AppExited { id, success });
            })?;
        Ok(())
    }
}
