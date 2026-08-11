//! The workspace's single source of cryptographic-quality entropy.
//!
//! Centralising this keeps one auditable answer to "where does randomness come
//! from" instead of a per-crate assortment of counters and clock readings, both
//! of which are predictable and have caused real protocol and identifier bugs.

use std::fmt;

/// The operating system's entropy source was unavailable.
///
/// On every platform OBS-RS targets this cannot happen once userspace is
/// running, so a failure means the process is in a state where continuing is
/// not meaningful — it is surfaced rather than papered over with a fallback
/// that would silently be predictable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RandomError;

impl fmt::Display for RandomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the operating system entropy source is unavailable")
    }
}

impl std::error::Error for RandomError {}

/// Fills `bytes` with entropy from the operating system.
///
/// # Errors
///
/// Returns [`RandomError`] when the platform entropy source cannot be read.
pub fn fill_random(bytes: &mut [u8]) -> Result<(), RandomError> {
    getrandom::fill(bytes).map_err(|_| RandomError)
}

/// Returns a random `u64` drawn from the operating system.
///
/// # Errors
///
/// Returns [`RandomError`] when the platform entropy source cannot be read.
pub fn random_u64() -> Result<u64, RandomError> {
    let mut bytes = [0_u8; 8];
    fill_random(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// A thread-local pool that amortizes entropy syscalls over many small draws.
///
/// Reading the OS entropy source once per WebSocket frame would put a syscall
/// on the per-frame send path. The pool refills in one call and hands out
/// slices of genuine OS entropy, so callers keep the security property without
/// paying for it per frame. Consumed bytes are zeroed so a pool left in memory
/// does not retain values already put on the wire.
pub struct RandomPool<const N: usize> {
    buffer: [u8; N],
    used: usize,
}

impl<const N: usize> Default for RandomPool<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RandomPool<N> {
    /// Creates an empty pool; the first draw fills it.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0_u8; N],
            used: N,
        }
    }

    /// Returns the next `M` random bytes, refilling from the OS when needed.
    ///
    /// # Errors
    ///
    /// Returns [`RandomError`] when a refill cannot read the entropy source.
    pub fn next_bytes<const M: usize>(&mut self) -> Result<[u8; M], RandomError> {
        // A draw wider than the pool would never fit; serve it straight from
        // the OS rather than silently truncating.
        if M > N {
            let mut bytes = [0_u8; M];
            fill_random(&mut bytes)?;
            return Ok(bytes);
        }

        if self.used + M > N {
            fill_random(&mut self.buffer)?;
            self.used = 0;
        }

        let mut bytes = [0_u8; M];
        bytes.copy_from_slice(&self.buffer[self.used..self.used + M]);
        self.buffer[self.used..self.used + M].fill(0);
        self.used += M;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn successive_draws_differ() {
        // Sixteen independent 8-byte draws colliding has probability around
        // 2^-58, so a repeat here means the source is not random at all.
        let values = (0..16)
            .map(|_| random_u64().expect("entropy is available"))
            .collect::<HashSet<_>>();

        assert_eq!(values.len(), 16);
    }

    #[test]
    fn a_pool_serves_many_draws_and_refills_transparently() {
        let mut pool = RandomPool::<32>::new();

        // Far more draws than the pool holds, forcing several refills.
        let masks = (0..64)
            .map(|_| pool.next_bytes::<4>().expect("entropy is available"))
            .collect::<HashSet<_>>();

        assert!(masks.len() > 56, "expected mostly distinct masks");
    }

    #[test]
    fn a_pool_serves_draws_wider_than_itself() {
        let mut pool = RandomPool::<4>::new();

        let first = pool.next_bytes::<16>().expect("entropy is available");
        let second = pool.next_bytes::<16>().expect("entropy is available");

        assert_ne!(first, second);
    }

    #[test]
    fn a_pool_zeroes_bytes_it_has_handed_out() {
        let mut pool = RandomPool::<16>::new();
        let issued = pool.next_bytes::<8>().expect("entropy is available");

        assert_eq!(&pool.buffer[..8], &[0; 8], "issued bytes are not retained");
        assert_ne!(issued, [0; 8], "a real draw is not all zeroes");
    }
}
