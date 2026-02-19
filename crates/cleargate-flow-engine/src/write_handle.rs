//! Dual-channel event dispatch for the write pipeline.
//!
//! The executor uses [`WriteHandle`] to emit [`WriteEvent`]s. Each event is
//! assigned a monotonic sequence number before dispatch.
//!
//! **Critical channel**: bounded, async send (backpressure — blocks if full).
//! Used for authoritative events that must never be lost.
//!
//! **Advisory channel**: bounded, `try_send` (drops if full).
//! Used for diagnostic events that are safe to lose under load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::mpsc;

use super::write_event::WriteEvent;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from [`WriteHandle`] operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteHandleError {
    /// The receiving end of the channel was dropped.
    #[error("write channel closed")]
    ChannelClosed,
}

// ---------------------------------------------------------------------------
// WriteHandle
// ---------------------------------------------------------------------------

/// Dual-channel event dispatch with monotonic sequence numbering.
///
/// All clones share the same `AtomicU64` counter, so sequence numbers are
/// globally unique and strictly increasing across the entire run.
#[derive(Clone)]
pub struct WriteHandle {
    critical_tx: mpsc::Sender<WriteEvent>,
    advisory_tx: mpsc::Sender<WriteEvent>,
    seq: Arc<AtomicU64>,
}

/// Receiving ends of the dual channels, consumed by the write pipeline.
pub struct WriteHandleReceivers {
    pub critical_rx: mpsc::Receiver<WriteEvent>,
    pub advisory_rx: mpsc::Receiver<WriteEvent>,
}

impl WriteHandle {
    /// Create a new `WriteHandle` and its corresponding receivers.
    ///
    /// `critical_capacity` and `advisory_capacity` set the bounded channel
    /// sizes. The critical channel blocks senders when full (backpressure);
    /// the advisory channel drops events when full.
    pub fn new(critical_capacity: usize, advisory_capacity: usize) -> (Self, WriteHandleReceivers) {
        let (critical_tx, critical_rx) = mpsc::channel(critical_capacity);
        let (advisory_tx, advisory_rx) = mpsc::channel(advisory_capacity);

        let handle = Self {
            critical_tx,
            advisory_tx,
            seq: Arc::new(AtomicU64::new(1)),
        };

        let receivers = WriteHandleReceivers {
            critical_rx,
            advisory_rx,
        };

        (handle, receivers)
    }

    /// Emit an authoritative event through the critical channel.
    ///
    /// Assigns the next monotonic sequence number. Blocks (async) if the
    /// channel is full — this is backpressure by design so authoritative
    /// data is never dropped.
    pub async fn write(&self, mut event: WriteEvent) -> Result<(), WriteHandleError> {
        event.set_seq(self.next_seq());
        self.critical_tx
            .send(event)
            .await
            .map_err(|_| WriteHandleError::ChannelClosed)
    }

    /// Emit a diagnostic event through the advisory channel.
    ///
    /// Assigns the next monotonic sequence number. Returns immediately —
    /// if the channel is full the event is silently dropped. This is
    /// acceptable for diagnostic data.
    pub fn try_write(&self, mut event: WriteEvent) -> Result<(), WriteHandleError> {
        event.set_seq(self.next_seq());
        // try_send returns Err on both Full and Closed. We only surface
        // ChannelClosed; Full is an expected (acceptable) outcome.
        match self.advisory_tx.try_send(event) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(WriteHandleError::ChannelClosed),
            Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
        }
    }

    /// Allocate the next monotonic sequence number.
    ///
    /// Exposed for cases where the executor needs to pre-assign a seq
    /// number before constructing the event.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TriggerSource, WRITE_EVENT_SCHEMA_VERSION};
    use chrono::Utc;
    use serde_json::json;

    fn make_event() -> WriteEvent {
        WriteEvent::NodeStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            node_id: "n".into(),
            inputs: json!({}),
            fan_out_index: None,
            attempt: 1,
            timestamp: Utc::now(),
        }
    }

    fn make_custom_event() -> WriteEvent {
        WriteEvent::Custom {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            node_id: None,
            name: "debug".into(),
            data: json!({}),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_monotonic_seq() {
        let (handle, mut receivers) = WriteHandle::new(1000, 1000);

        for _ in 0..100 {
            handle.write(make_event()).await.unwrap();
        }

        let mut prev = 0u64;
        for _ in 0..100 {
            let event = receivers.critical_rx.recv().await.unwrap();
            let seq = event.seq();
            assert!(seq > prev, "seq {seq} must be > prev {prev}");
            prev = seq;
        }
    }

    #[tokio::test]
    async fn test_critical_blocks() {
        // Channel capacity of 2 — fill it, then verify the third write blocks.
        let (handle, _receivers) = WriteHandle::new(2, 2);

        handle.write(make_event()).await.unwrap();
        handle.write(make_event()).await.unwrap();

        // Third write should block (channel full, receiver not draining).
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            handle.write(make_event()),
        )
        .await;

        assert!(result.is_err(), "write should have timed out (blocked)");
    }

    #[tokio::test]
    async fn test_advisory_drops() {
        // Channel capacity of 2 — fill it, then verify try_write returns Ok
        // but the event is dropped (not an error).
        let (handle, mut receivers) = WriteHandle::new(100, 2);

        handle.try_write(make_custom_event()).unwrap();
        handle.try_write(make_custom_event()).unwrap();

        // Channel is now full — this should return Ok (event dropped silently).
        let result = handle.try_write(make_custom_event());
        assert!(result.is_ok(), "try_write should succeed even when full");

        // Only 2 events should be in the channel.
        let mut count = 0;
        while receivers.advisory_rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_clone_shares_seq() {
        let (handle_a, mut receivers) = WriteHandle::new(1000, 1000);
        let handle_b = handle_a.clone();

        // Interleave writes from both handles.
        handle_a.write(make_event()).await.unwrap();
        handle_b.write(make_event()).await.unwrap();
        handle_a.write(make_event()).await.unwrap();
        handle_b.write(make_event()).await.unwrap();

        let mut seen = std::collections::HashSet::new();
        for _ in 0..4 {
            let event = receivers.critical_rx.recv().await.unwrap();
            assert!(seen.insert(event.seq()), "duplicate seq: {}", event.seq());
        }
        assert_eq!(seen.len(), 4);
    }

    #[tokio::test]
    async fn test_critical_never_loses() {
        let (handle, mut receivers) = WriteHandle::new(2000, 100);

        for _ in 0..1000 {
            handle.write(make_event()).await.unwrap();
        }

        drop(handle);

        let mut count = 0;
        while receivers.critical_rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 1000);
    }

    #[tokio::test]
    async fn test_write_assigns_seq() {
        let (handle, mut receivers) = WriteHandle::new(100, 100);

        // Event starts with seq=0.
        let event = WriteEvent::RunStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "r".into(),
            flow_id: "f".into(),
            version_id: "v".into(),
            trigger_inputs: json!({}),
            triggered_by: TriggerSource::Manual {
                principal: "test".into(),
            },
            config_hash: None,
            parent_run_id: None,
            timestamp: Utc::now(),
        };
        assert_eq!(event.seq(), 0);

        handle.write(event).await.unwrap();

        let received = receivers.critical_rx.recv().await.unwrap();
        assert!(received.seq() > 0, "seq should be assigned by WriteHandle");
    }
}
