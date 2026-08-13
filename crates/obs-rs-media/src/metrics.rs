use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const LATENCY_BUCKETS: usize = 16;
const FIRST_LATENCY_BUCKET_BITS: u32 = 16;

/// Fixed-memory logarithmic latency distribution in nanoseconds.
///
/// Buckets are powers of two, so percentile values are conservative upper
/// bounds with no retained samples and no allocation in [`Self::record`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatencyMetrics {
    buckets: [u16; LATENCY_BUCKETS],
    samples: u32,
    max_nanos: u64,
}

impl Default for LatencyMetrics {
    fn default() -> Self {
        Self {
            buckets: [0; LATENCY_BUCKETS],
            samples: 0,
            max_nanos: 0,
        }
    }
}

impl LatencyMetrics {
    /// Records one duration into a bounded logarithmic bucket.
    pub fn record(&mut self, duration: Duration) {
        self.record_nanos(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
    }

    /// Records one latency expressed in nanoseconds.
    pub fn record_nanos(&mut self, nanos: u64) {
        let significant_bits = u64::BITS - nanos.leading_zeros();
        let bucket = usize::try_from(significant_bits.saturating_sub(FIRST_LATENCY_BUCKET_BITS))
            .unwrap_or(LATENCY_BUCKETS - 1)
            .min(LATENCY_BUCKETS - 1);
        if self.buckets[bucket] == u16::MAX {
            self.rescale();
        }
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.samples = self.samples.saturating_add(1);
        self.max_nanos = self.max_nanos.max(nanos);
    }

    /// Number of observations represented by this distribution.
    #[must_use]
    pub const fn samples(self) -> u32 {
        self.samples
    }

    /// Conservative percentile upper bound in nanoseconds.
    #[must_use]
    pub fn percentile_nanos(self, percentile: u8) -> u64 {
        if self.samples == 0 {
            return 0;
        }
        let percentile = u64::from(percentile.clamp(1, 100));
        let target = u64::from(self.samples)
            .saturating_mul(percentile)
            .div_ceil(100);
        let mut seen = 0_u64;
        for (bucket, count) in self.buckets.into_iter().enumerate() {
            seen = seen.saturating_add(u64::from(count));
            if seen >= target {
                return if bucket == LATENCY_BUCKETS - 1 {
                    self.max_nanos
                } else {
                    bucket_upper_bound(bucket)
                };
            }
        }
        self.max_nanos
    }

    /// Largest exact observation in nanoseconds.
    #[must_use]
    pub const fn max_nanos(self) -> u64 {
        self.max_nanos
    }

    fn rescale(&mut self) {
        self.samples = 0;
        for count in &mut self.buckets {
            *count = (*count / 2).max(u16::from(*count > 0));
            self.samples = self.samples.saturating_add(u32::from(*count));
        }
    }
}

fn bucket_upper_bound(bucket: usize) -> u64 {
    let bits = u32::try_from(bucket)
        .unwrap_or(u32::MAX)
        .saturating_add(FIRST_LATENCY_BUCKET_BITS);
    1_u64.checked_shl(bits).map_or(u64::MAX, |value| value - 1)
}

static OWNED_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static OWNED_BYTES: AtomicUsize = AtomicUsize::new(0);
static SHARED_CLONES: AtomicUsize = AtomicUsize::new(0);
static COPY_ON_WRITE_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static COPY_ON_WRITE_BYTES: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Per-thread mirror of the process-wide counters.
    ///
    /// The global counters describe the whole process, so any concurrently
    /// rendering thread perturbs them. Callers that need to attribute buffer
    /// activity to exactly one workload sample this instead.
    static LOCAL: Cell<FrameMemoryMetrics> = const { Cell::new(FrameMemoryMetrics::ZERO) };
}

/// Process-wide counters for frame-storage activity.
///
/// These counters deliberately describe media-buffer operations rather than
/// allocator internals: they are portable, deterministic, and useful in
/// benchmarks without installing a global allocator. Values are cumulative
/// until [`reset_frame_memory_metrics`] is called.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameMemoryMetrics {
    owned_buffers: usize,
    owned_bytes: usize,
    shared_clones: usize,
    copy_on_write_buffers: usize,
    copy_on_write_bytes: usize,
}

impl FrameMemoryMetrics {
    /// All counters at zero.
    const ZERO: Self = Self {
        owned_buffers: 0,
        owned_bytes: 0,
        shared_clones: 0,
        copy_on_write_buffers: 0,
        copy_on_write_bytes: 0,
    };

    /// Number of owned pixel buffers accepted or allocated by `VideoFrame`.
    #[must_use]
    pub const fn owned_buffers(self) -> usize {
        self.owned_buffers
    }

    /// Total bytes represented by newly owned frame buffers.
    #[must_use]
    pub const fn owned_bytes(self) -> usize {
        self.owned_bytes
    }

    /// Number of frame clones that shared immutable pixel storage.
    #[must_use]
    pub const fn shared_clones(self) -> usize {
        self.shared_clones
    }

    /// Number of shared buffers copied before an in-place mutation.
    #[must_use]
    pub const fn copy_on_write_buffers(self) -> usize {
        self.copy_on_write_buffers
    }

    /// Total bytes copied by copy-on-write frame mutations.
    #[must_use]
    pub const fn copy_on_write_bytes(self) -> usize {
        self.copy_on_write_bytes
    }
}

/// Returns a consistent-enough snapshot of process-wide frame-buffer counters.
///
/// Each counter is atomic, although a concurrently rendering thread can advance
/// one counter between loads. Benchmarks should reset and sample while they own
/// the measured workload.
#[must_use]
pub fn frame_memory_metrics() -> FrameMemoryMetrics {
    FrameMemoryMetrics {
        owned_buffers: OWNED_BUFFERS.load(Ordering::Relaxed),
        owned_bytes: OWNED_BYTES.load(Ordering::Relaxed),
        shared_clones: SHARED_CLONES.load(Ordering::Relaxed),
        copy_on_write_buffers: COPY_ON_WRITE_BUFFERS.load(Ordering::Relaxed),
        copy_on_write_bytes: COPY_ON_WRITE_BYTES.load(Ordering::Relaxed),
    }
}

/// Resets process-wide frame-buffer counters before a controlled measurement.
pub fn reset_frame_memory_metrics() {
    OWNED_BUFFERS.store(0, Ordering::Relaxed);
    OWNED_BYTES.store(0, Ordering::Relaxed);
    SHARED_CLONES.store(0, Ordering::Relaxed);
    COPY_ON_WRITE_BUFFERS.store(0, Ordering::Relaxed);
    COPY_ON_WRITE_BYTES.store(0, Ordering::Relaxed);
}

/// Returns frame-buffer counters for the calling thread only.
///
/// Unlike [`frame_memory_metrics`], this is unaffected by work on other
/// threads, so a test or benchmark can measure one workload exactly even while
/// the rest of the process renders. Note that frame operations large enough to
/// run in parallel attribute their work to the rayon worker threads that
/// performed it, not to the thread that started them.
#[must_use]
pub fn thread_frame_memory_metrics() -> FrameMemoryMetrics {
    LOCAL.with(Cell::get)
}

/// Resets the calling thread's frame-buffer counters.
pub fn reset_thread_frame_memory_metrics() {
    LOCAL.with(|local| local.set(FrameMemoryMetrics::ZERO));
}

/// Applies `update` to the calling thread's counters.
fn record_local(update: impl FnOnce(&mut FrameMemoryMetrics)) {
    LOCAL.with(|local| {
        let mut metrics = local.get();
        update(&mut metrics);
        local.set(metrics);
    });
}

pub(crate) fn record_owned_buffer(bytes: usize) {
    OWNED_BUFFERS.fetch_add(1, Ordering::Relaxed);
    OWNED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    record_local(|metrics| {
        metrics.owned_buffers = metrics.owned_buffers.saturating_add(1);
        metrics.owned_bytes = metrics.owned_bytes.saturating_add(bytes);
    });
}

pub(crate) fn record_shared_clone() {
    SHARED_CLONES.fetch_add(1, Ordering::Relaxed);
    record_local(|metrics| {
        metrics.shared_clones = metrics.shared_clones.saturating_add(1);
    });
}

pub(crate) fn record_copy_on_write(bytes: usize) {
    COPY_ON_WRITE_BUFFERS.fetch_add(1, Ordering::Relaxed);
    COPY_ON_WRITE_BYTES.fetch_add(bytes, Ordering::Relaxed);
    record_local(|metrics| {
        metrics.copy_on_write_buffers = metrics.copy_on_write_buffers.saturating_add(1);
        metrics.copy_on_write_bytes = metrics.copy_on_write_bytes.saturating_add(bytes);
    });
}

#[cfg(test)]
mod latency_tests {
    use super::*;

    #[test]
    fn latency_percentiles_are_bounded_conservative_and_allocation_free() {
        let mut metrics = LatencyMetrics::default();
        for nanos in [1, 2, 3, 4, 100] {
            metrics.record_nanos(nanos);
        }
        assert_eq!(metrics.samples(), 5);
        assert_eq!(metrics.percentile_nanos(50), 65_535);
        assert_eq!(metrics.percentile_nanos(95), 65_535);
        assert_eq!(metrics.percentile_nanos(99), 65_535);
        assert_eq!(metrics.max_nanos(), 100);
    }
}
