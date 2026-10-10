// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC server: accepts connections, turns what clients say into [`Event`]s and lets the
//! runtime talk back to the app. Every thread here blocks in a read or an accept; none polls.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use vixeeny_common::ipc::{
    self, AppToDaemon, ControlReply, DaemonToApp, Hello, IpcError, Listener, ListenerTrait,
    SendHalf, Stream, StreamTrait,
};

use crate::core::Event;

/// What the platform layer provides so that other threads can interrupt its message loop.
pub trait Waker: Send + Sync {
    fn wake(&self);
}

/// Sends events to the main thread and wakes it.
#[derive(Clone)]
pub struct EventTx {
    tx: Sender<Event>,
    waker: Arc<dyn Waker>,
}

impl EventTx {
    pub fn new(tx: Sender<Event>, waker: Arc<dyn Waker>) -> Self {
        Self { tx, waker }
    }

    /// The receiver being gone means the daemon is shutting down; nothing to report.
    pub fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            self.waker.wake();
        }
    }
}

struct Active {
    id: u64,
    send: SendHalf,
}

/// The write side of the connection to the running app, if any.
#[derive(Clone, Default)]
pub struct AppLink {
    active: Arc<Mutex<Option<Active>>>,
}

impl AppLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn install(&self, id: u64, send: SendHalf) {
        *self.lock() = Some(Active { id, send });
    }

    /// Drops the link if `id` is still the current connection.
    fn clear_if_current(&self, id: u64) -> bool {
        let mut guard = self.lock();
        if guard.as_ref().is_some_and(|a| a.id == id) {
            *guard = None;
            true
        } else {
            false
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.lock().as_ref().is_some_and(|a| a.id == id)
    }

    /// Sends a message to the app; `Ok(false)` if no app is connected.
    pub fn send(&self, msg: &DaemonToApp) -> Result<bool, IpcError> {
        match self.lock().as_mut() {
            Some(active) => ipc::write_msg(&mut active.send, msg).map(|()| true),
            None => Ok(false),
        }
    }
}

/// Accepts connections until the listener fails. Meant to run on its own thread.
pub fn serve(listener: &Listener, tx: &EventTx, link: &AppLink) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!("ipc accept failed: {e}");
                // A persistent failure would spin; give the OS a moment.
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, link) = (tx.clone(), link.clone());
        let spawned = std::thread::Builder::new()
            .name(format!("ipc-{id}"))
            .stack_size(128 * 1024)
            .spawn(move || {
                if let Err(e) = connection(stream, id, &tx, &link) {
                    tracing::debug!("ipc connection {id} ended: {e}");
                }
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start an ipc thread: {e}");
        }
    }
}

fn connection(stream: Stream, id: u64, tx: &EventTx, link: &AppLink) -> Result<(), IpcError> {
    let (mut recv, mut send) = stream.split();
    let Some(hello) = ipc::read_msg::<_, Hello>(&mut recv)? else {
        return Ok(());
    };
    match hello {
        Hello::Control(request) => {
            tx.send(Event::Control(request));
            ipc::write_msg(&mut send, &ControlReply::Ok)
        }
        Hello::App { pid } => {
            tracing::debug!("app connected (pid {pid}, connection {id})");
            link.install(id, send);
            tx.send(Event::AppConnected);
            let result = loop {
                match ipc::read_msg::<_, AppToDaemon>(&mut recv) {
                    Ok(Some(msg)) => {
                        // A newer app replaced this connection: ignore the stale one.
                        if !link.is_current(id) {
                            break Ok(());
                        }
                        tx.send(Event::App(msg));
                    }
                    Ok(None) => break Ok(()),
                    Err(e) => break Err(e),
                }
            };
            if link.clear_if_current(id) {
                tx.send(Event::AppDisconnected);
            }
            result
        }
    }
}

/// Tells a running daemon to open the settings (second launch). Returns the I/O result.
pub fn ask_running_daemon(
    endpoint: &ipc::Endpoint,
    request: ipc::ControlRequest,
) -> io::Result<()> {
    let mut stream = endpoint.connect()?;
    ipc::write_msg(&mut stream, &Hello::Control(request)).map_err(io::Error::other)?;
    ipc::read_msg::<_, ControlReply>(&mut stream)
        .map(|_| ())
        .map_err(io::Error::other)
}
