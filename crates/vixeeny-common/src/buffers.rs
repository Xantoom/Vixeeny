// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame-sized byte buffers handed back once used, so that a stream of frames reuses the same memory instead of asking
//! the system for megabytes (and faulting them in) at every frame.

use std::sync::{Arc, Mutex};

/// Spare buffers kept: what one frame in flight and one being converted need. More would only hold memory after a
/// burst.
const SPARE: usize = 2;

/// Shared between the producer (`take`) and the consumer (`give`); cloning shares the pool.
#[derive(Debug, Clone, Default)]
pub struct BufferPool(Arc<Mutex<Vec<Vec<u8>>>>);

impl BufferPool {
    /// An empty buffer that can hold `len` bytes without growing.
    pub fn take(&self, len: usize) -> Vec<u8> {
        let spare = self.0.lock().ok().and_then(|mut spare| spare.pop());
        let mut buffer = spare.unwrap_or_default();
        // A resized window must not retain two full-screen allocations for the rest of a recording. Hysteresis
        // avoids reallocating for small size changes; take(0), used to inspect a spare, keeps its allocation.
        if len > 0 && buffer.capacity() > len.saturating_mul(4).max(1 << 20) {
            buffer = Vec::new();
        }
        buffer.clear();
        buffer.reserve(len);
        buffer
    }

    /// Hands `buffer` back for a later `take`.
    pub fn give(&self, buffer: Vec<u8>) {
        if let Ok(mut spare) = self.0.lock()
            && spare.len() < SPARE
        {
            spare.push(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_much_smaller_stream_releases_oversized_spares_but_small_changes_reuse_them() {
        let pool = BufferPool::default();
        let large = pool.take(8 << 20);
        let address = large.as_ptr();
        pool.give(large);
        let similar = pool.take(4 << 20);
        assert_eq!(similar.as_ptr(), address);
        pool.give(similar);
        let small = pool.take(64 << 10);
        assert!(small.capacity() >= 64 << 10);
        assert!(small.capacity() <= 1 << 20);
    }

    #[test]
    fn a_stream_of_frames_reuses_the_same_memory_and_keeps_little() {
        let pool = BufferPool::default();
        let mut first = pool.take(1000);
        first.extend_from_slice(&[1; 1000]);
        let address = first.as_ptr();
        pool.give(first);
        let again = pool.take(1000);
        assert_eq!(again.as_ptr(), address);
        assert!(again.is_empty() && again.capacity() >= 1000);
        for _ in 0..5 {
            pool.give(vec![0; 10]);
        }
        assert_eq!(pool.0.lock().unwrap().len(), SPARE);
    }
}
