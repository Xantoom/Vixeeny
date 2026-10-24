// SPDX-License-Identifier: GPL-3.0-or-later
//! The capture backend of this process (a Direct3D device: making one takes longer than a
//! capture), kept from one capture to the next until the process ends once idle, and made in
//! the background while the app starts for a capture.

use std::ops::{Deref, DerefMut};
use std::sync::Mutex;
use std::thread::JoinHandle;

use vixeeny_capture::{CaptureError, StillBackend, WgcBackend};

/// The kept backend, between captures (any thread may take it: the scrolling capture runs on
/// its own).
static KEPT: Mutex<Option<WgcBackend>> = Mutex::new(None);

/// The backend being made in the background.
static WARMING: Mutex<Option<JoinHandle<Result<WgcBackend, CaptureError>>>> = Mutex::new(None);

/// Starts making the backend in the background, unless it is kept or being made already.
pub fn warm() {
    let kept = KEPT.lock().is_ok_and(|k| k.is_some());
    let warming = WARMING.lock().is_ok_and(|w| w.is_some());
    if kept || warming {
        return;
    }
    let made = std::thread::Builder::new()
        .name("capture-warm".into())
        .spawn(WgcBackend::new);
    if let (Ok(handle), Ok(mut warming)) = (made, WARMING.lock()) {
        *warming = Some(handle);
    }
}

/// The backend, lent: it goes back to be kept when dropped, unless [`discard`](Self::discard)ed.
pub struct Lent {
    backend: Option<WgcBackend>,
    /// It was kept from an earlier capture (rather than made for this one): if it fails, the
    /// device may be gone (a driver update, the computer asleep).
    pub kept: bool,
}

impl Lent {
    /// The kept backend, the one made in the background, or a new one.
    pub fn take() -> Result<Self, CaptureError> {
        let kept = KEPT.lock().ok().and_then(|mut k| k.take());
        if let Some(backend) = kept.filter(|b| !b.is_lost()) {
            return Ok(Self {
                backend: Some(backend),
                kept: true,
            });
        }
        let warming = WARMING.lock().ok().and_then(|mut w| w.take());
        let backend = match warming.map(JoinHandle::join) {
            Some(Ok(Ok(backend))) => backend,
            _ => WgcBackend::new()?,
        };
        Ok(Self {
            backend: Some(backend),
            kept: false,
        })
    }

    /// A new backend in place of this one, which failed.
    pub fn renew(&mut self) -> Result<(), CaptureError> {
        self.backend = Some(WgcBackend::new()?);
        self.kept = false;
        Ok(())
    }

    /// Not kept: it failed, the next capture makes a new one.
    pub fn discard(mut self) {
        self.backend = None;
    }
}

impl Deref for Lent {
    type Target = WgcBackend;
    fn deref(&self) -> &WgcBackend {
        self.backend
            .as_ref()
            .unwrap_or_else(|| unreachable!("lent until dropped"))
    }
}

impl DerefMut for Lent {
    fn deref_mut(&mut self) -> &mut WgcBackend {
        self.backend
            .as_mut()
            .unwrap_or_else(|| unreachable!("lent until dropped"))
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        // Kept for the next capture, without the memory of this one.
        if let Some(device) = self.backend.as_ref().and_then(|b| b.d3d_device()) {
            vixeeny_capture::trim(&device);
        }
        if let (Some(backend), Ok(mut kept)) = (self.backend.take(), KEPT.lock()) {
            *kept = Some(backend);
        }
    }
}
