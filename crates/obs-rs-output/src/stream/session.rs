use crate::{
    error::OutputError,
    queue::PacketQueue,
    types::{
        EncodedPacket, OutputState, PacketDropPolicy, PacketPushOutcome, ReconnectOutcome,
        ReconnectPolicy, StreamMetrics, StreamState,
    },
};
use std::{sync::Arc, time::Instant};

pub trait PacketMuxer {
    /// Accepts one packet in timestamp order.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the session is closed, the packet limit is
    /// exceeded, or the packet cannot be represented.
    fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError>;

    /// Atomically commits all accepted packets and returns the final bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the session is not open or serialization
    /// fails.
    fn finalize(&mut self) -> Result<Arc<Vec<u8>>, OutputError>;

    /// Cancels the session and discards uncommitted data.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    fn abort(&mut self) -> Result<(), OutputError>;

    /// Returns the current lifecycle state.
    fn state(&self) -> OutputState;
}

/// Transport boundary used by the streaming session.
pub trait PacketTransport {
    /// Establishes or re-establishes the transport connection.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Transport`] when the connection cannot be opened.
    fn connect(&mut self) -> Result<(), OutputError>;

    /// Sends one packet without taking ownership of the queued copy.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Transport`] when delivery fails. The stream session
    /// re-queues the packet before returning the error.
    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError>;

    /// Sends a run of packets, returning how many were delivered.
    ///
    /// Transports backed by a socket should override this to coalesce the run
    /// into as few writes as possible; the default sends one at a time and stops
    /// at the first failure so the caller can re-queue the remainder.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Transport`] from the first failed packet, after
    /// reporting the count that did succeed via `delivered`.
    fn send_batch(
        &mut self,
        packets: &[EncodedPacket],
        delivered: &mut usize,
    ) -> Result<(), OutputError> {
        for packet in packets {
            self.send(packet)?;
            *delivered += 1;
        }
        Ok(())
    }

    /// Closes the transport without changing queued packet ownership.
    fn disconnect(&mut self);
}

/// A bounded, reconnectable packet stream session.
pub struct StreamSession<T: PacketTransport> {
    transport: T,
    queue: PacketQueue,
    reconnect_policy: ReconnectPolicy,
    reconnect_attempts: u32,
    next_reconnect_at: Option<Instant>,
    state: StreamState,
    metrics: StreamMetrics,
}

impl<T: PacketTransport> StreamSession<T> {
    /// Creates a disconnected stream session with bounded packet storage.
    ///
    /// # Errors
    ///
    /// Returns queue-capacity errors from [`PacketQueue::new`].
    pub fn new(
        transport: T,
        capacity_bytes: usize,
        drop_policy: PacketDropPolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Result<Self, OutputError> {
        Ok(Self {
            transport,
            queue: PacketQueue::new(capacity_bytes, drop_policy)?,
            reconnect_policy,
            reconnect_attempts: 0,
            next_reconnect_at: None,
            state: StreamState::Disconnected,
            metrics: StreamMetrics::default(),
        })
    }

    /// Connects the transport for the first time.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] for a closed or failed stream, or
    /// [`OutputError::Transport`] when the transport rejects the connection.
    pub fn connect(&mut self) -> Result<(), OutputError> {
        self.ensure_connectable("connect")?;
        self.transport.connect()?;
        self.next_reconnect_at = None;
        self.state = StreamState::Connected;
        Ok(())
    }

    /// Queues one packet without blocking on the transport.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] for a closed or failed stream, or a
    /// queue-capacity error.
    pub fn submit(&mut self, packet: EncodedPacket) -> Result<PacketPushOutcome, OutputError> {
        if matches!(self.state, StreamState::Closed | StreamState::Failed) {
            return Err(OutputError::InvalidStreamState {
                operation: "submit a stream packet",
                state: self.state,
            });
        }
        let outcome = self.queue.push(packet)?;
        self.metrics.submitted = self.metrics.submitted.saturating_add(1);
        match outcome {
            PacketPushOutcome::DroppedOldest { packets, .. } => {
                self.metrics.dropped_packets =
                    self.metrics.dropped_packets.saturating_add(packets as u64);
            }
            PacketPushOutcome::DroppedNewest { .. } => {
                self.metrics.dropped_packets = self.metrics.dropped_packets.saturating_add(1);
            }
            PacketPushOutcome::Enqueued => {}
        }
        Ok(outcome)
    }

    /// Sends all currently queued packets.
    ///
    /// A failed packet is put back at the front before the error is returned, so
    /// reconnecting can retry it without blocking the producer.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] while disconnected, or the transport
    /// error from the failed send.
    pub fn flush(&mut self) -> Result<usize, OutputError> {
        if self.state != StreamState::Connected {
            return Err(OutputError::InvalidStreamState {
                operation: "flush stream packets",
                state: self.state,
            });
        }
        // Drain once and submit the whole run, so a socket transport can
        // coalesce it into a single write instead of one syscall per packet.
        let mut batch = Vec::with_capacity(self.queue.len());
        while let Some(packet) = self.queue.pop() {
            batch.push(packet);
        }

        let mut sent = 0;
        let result = self.transport.send_batch(&batch, &mut sent);
        self.metrics.sent_packets = self.metrics.sent_packets.saturating_add(sent as u64);

        if let Err(error) = result {
            self.metrics.send_failures = self.metrics.send_failures.saturating_add(1);
            self.state = StreamState::Disconnected;
            // Re-queue the undelivered tail in its original order. The
            // reversal is required, not redundant: `push_front` prepends and
            // `pop` takes from the front, so walking the tail newest-first is
            // what leaves the queue oldest-first. Dropping the `.rev()` sends
            // the tail back to front and breaks monotonic delivery.
            for packet in batch.into_iter().skip(sent).rev() {
                self.queue.push_front(packet)?;
            }
            self.schedule_reconnect(Instant::now());
            return Err(error);
        }
        Ok(sent)
    }

    /// Attempts one reconnect under the configured budget.
    ///
    /// # Errors
    ///
    /// Returns the transport error when this attempt fails, or
    /// [`OutputError::ReconnectExhausted`] after the limit is reached.
    pub fn reconnect(&mut self) -> Result<ReconnectOutcome, OutputError> {
        self.reconnect_at(Instant::now())
    }

    pub(crate) fn reconnect_at(&mut self, now: Instant) -> Result<ReconnectOutcome, OutputError> {
        self.ensure_connectable("reconnect")?;
        if self.reconnect_attempts >= self.reconnect_policy.max_attempts() {
            self.state = StreamState::Failed;
            return Err(OutputError::ReconnectExhausted {
                attempts: self.reconnect_attempts,
            });
        }
        if let Some(deadline) = self.next_reconnect_at {
            if now < deadline {
                return Ok(ReconnectOutcome::Deferred {
                    retry_after: deadline.duration_since(now),
                });
            }
        }
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        match self.transport.connect() {
            Ok(()) => {
                self.next_reconnect_at = None;
                self.state = StreamState::Connected;
                self.metrics.reconnects = self.metrics.reconnects.saturating_add(1);
                Ok(ReconnectOutcome::Reconnected)
            }
            Err(error) => {
                self.schedule_reconnect(now);
                if self.reconnect_attempts >= self.reconnect_policy.max_attempts() {
                    self.state = StreamState::Failed;
                }
                Err(error)
            }
        }
    }

    /// Disconnects while preserving queued packets for a later reconnect.
    pub fn disconnect(&mut self) {
        self.disconnect_at(Instant::now());
    }

    pub(crate) fn disconnect_at(&mut self, now: Instant) {
        if self.state != StreamState::Closed {
            self.transport.disconnect();
            self.schedule_reconnect(now);
            self.state = StreamState::Disconnected;
        }
    }

    /// Permanently closes the stream and discards queued packets.
    pub fn close(&mut self) {
        if self.state != StreamState::Closed {
            self.transport.disconnect();
            self.queue.clear();
            self.state = StreamState::Closed;
        }
    }

    /// Returns the stream lifecycle state.
    #[must_use]
    pub const fn state(&self) -> StreamState {
        self.state
    }

    /// Returns the number of queued bytes.
    #[must_use]
    pub const fn queued_bytes(&self) -> usize {
        self.queue.queued_bytes()
    }

    /// Returns stream metrics.
    #[must_use]
    pub const fn metrics(&self) -> StreamMetrics {
        self.metrics
    }

    /// Returns the reconnect attempts consumed so far.
    #[must_use]
    pub const fn reconnect_attempts(&self) -> u32 {
        self.reconnect_attempts
    }

    /// Borrows the transport for inspection.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    fn ensure_connectable(&self, operation: &'static str) -> Result<(), OutputError> {
        match self.state {
            StreamState::Disconnected | StreamState::Connected => Ok(()),
            state => Err(OutputError::InvalidStreamState { operation, state }),
        }
    }

    fn schedule_reconnect(&mut self, now: Instant) {
        self.next_reconnect_at = now.checked_add(
            self.reconnect_policy
                .delay_for_attempt(self.reconnect_attempts),
        );
    }
}

impl<T: PacketTransport> super::StreamingTransport for StreamSession<T> {
    type Error = OutputError;

    fn poll(&mut self) -> Result<usize, Self::Error> {
        self.flush()
    }

    fn reconnect(&mut self) -> Result<ReconnectOutcome, Self::Error> {
        Self::reconnect(self)
    }

    fn close(&mut self) -> Result<(), Self::Error> {
        Self::close(self);
        Ok(())
    }
}
