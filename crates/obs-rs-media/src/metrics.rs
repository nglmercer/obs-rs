use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
};

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
