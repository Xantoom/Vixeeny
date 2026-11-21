// SPDX-License-Identifier: GPL-3.0-or-later
//! Waking the interface thread from another thread, without it looking from time to time: a [`Bell`] rung on any thread
//! runs, on the interface thread, what [`Bell::on_ring`] set.

use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

/// Shared between the thread that rings and the interface thread that listens.
#[derive(Clone, Default)]
pub struct Bell(Arc<Mutex<Rung>>);

#[derive(Default)]
struct Rung {
    rung: bool,
    /// No more rings: what listens ends.
    closed: bool,
    waker: Option<Waker>,
}

impl Bell {
    pub fn new() -> Self {
        Self::default()
    }

    /// From any thread.
    pub fn ring(&self) {
        let waker = self.0.lock().ok().and_then(|mut r| {
            r.rung = !r.closed;
            r.waker.take()
        });
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// From any thread: no more rings. What [`on_ring`](Self::on_ring) set ends, and with it what it holds (the
    /// window's state, for a window that closed).
    pub fn close(&self) {
        let waker = self.0.lock().ok().and_then(|mut r| {
            r.closed = true;
            r.rung = false;
            r.waker.take()
        });
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Ends at the next ring (rings meanwhile count as one): `true`, or `false` once closed.
    pub async fn rung(&self) -> bool {
        poll_fn(|cx| {
            let Ok(mut r) = self.0.lock() else {
                return Poll::Ready(false);
            };
            if r.closed {
                Poll::Ready(false)
            } else if std::mem::take(&mut r.rung) {
                Poll::Ready(true)
            } else {
                r.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }

    /// Runs `f` on this (the interface) thread after each ring, until [`close`](Self::close) or the end of its event
    /// loop.
    pub fn on_ring(&self, mut f: impl FnMut() + 'static) {
        let bell = self.clone();
        let _ = slint::spawn_local(async move {
            while bell.rung().await {
                f();
            }
        });
    }
}
