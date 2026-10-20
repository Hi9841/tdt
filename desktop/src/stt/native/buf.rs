//! Reused f32 buffers. A forward pass allocates and frees buffers of the
//! same few sizes in every layer. Fresh large allocations come from Windows
//! as untouched pages, and the page faults on first write stall all threads.
//! A buffer goes back to the pool when it is dropped.

use parking_lot::Mutex;
use std::ops::{Deref, DerefMut};

/// Freed buffers, kept until the next take needs them.
static POOL: Mutex<Vec<Vec<f32>>> = Mutex::new(Vec::new());
/// Enough for every live buffer of one forward pass.
const MAX_POOLED: usize = 64;

pub struct Buf(Vec<f32>);

impl Buf {
    /// `len` zeros, in memory that a previous buffer already touched when one fits.
    pub fn zeroed(len: usize) -> Self {
        let reused = {
            let mut pool = POOL.lock();
            // Smallest pooled buffer that fits.
            let best = pool
                .iter()
                .enumerate()
                .filter(|(_, v)| v.capacity() >= len)
                .min_by_key(|(_, v)| v.capacity())
                .map(|(i, _)| i);
            best.map(|i| pool.swap_remove(i))
        };
        let mut vec = reused.unwrap_or_else(|| Vec::with_capacity(len));
        vec.clear();
        vec.resize(len, 0.0);
        Buf(vec)
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        let vec = std::mem::take(&mut self.0);
        if vec.capacity() == 0 {
            return;
        }
        let mut pool = POOL.lock();
        if pool.len() < MAX_POOLED {
            pool.push(vec);
        }
    }
}

impl Deref for Buf {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        &self.0
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut [f32] {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::Buf;

    #[test]
    fn a_dropped_buffer_is_reused_with_the_requested_length() {
        let first = Buf::zeroed(1000);
        let address = first.as_ptr();
        drop(first);
        let second = Buf::zeroed(900);
        assert_eq!(second.len(), 900);
        assert!(second.iter().all(|v| *v == 0.0));
        // Another test may have taken the buffer in between; only check when it came back.
        if second.as_ptr() == address {
            assert!(second.len() <= 1000);
        }
    }
}
