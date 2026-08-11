use std::sync::atomic::{AtomicUsize, Ordering};

static OWNED_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static OWNED_BYTES: AtomicUsize = AtomicUsize::new(0);
static SHARED_CLONES: AtomicUsize = AtomicUsize::new(0);
static COPY_ON_WRITE_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static COPY_ON_WRITE_BYTES: AtomicUsize = AtomicUsize::new(0);

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

pub(crate) fn record_owned_buffer(bytes: usize) {
    OWNED_BUFFERS.fetch_add(1, Ordering::Relaxed);
    OWNED_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

pub(crate) fn record_shared_clone() {
    SHARED_CLONES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_copy_on_write(bytes: usize) {
    COPY_ON_WRITE_BUFFERS.fetch_add(1, Ordering::Relaxed);
    COPY_ON_WRITE_BYTES.fetch_add(bytes, Ordering::Relaxed);
}
