//! Typed request-event channel.
//!
//! `EventBus` fans events out via a `tokio::sync::broadcast` channel. Slow
//! subscribers are dropped on lag — telemetry is a side-channel and must
//! never back-pressure the request path. When `send` returns Err (no
//! subscribers, or — for `RecvError::Lagged` — observed by the receiver),
//! the publish path also bumps `submux_stream_backpressure_drops_total` so
//! the drop is observable.

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;

use crate::core::AccountId;
use crate::telemetry::metrics;

/// Default broadcast buffer capacity. 2048 entries is enough headroom for a
/// healthy admin UI / log shipper to catch up after a transient stall.
pub const DEFAULT_CAPACITY: usize = 2048;

/// Lifecycle events for a single gateway request. Cloneable so the broadcast
/// channel can fan out to N consumers cheaply (Strings are short).
#[derive(Debug, Clone)]
pub enum RequestEvent {
    Accepted {
        request_id: String,
        protocol: String,
        model_hint: Option<String>,
        timestamp: DateTime<Utc>,
    },
    AccountPicked {
        request_id: String,
        account_id: AccountId,
        provider: String,
        timestamp: DateTime<Utc>,
    },
    UpstreamStart {
        request_id: String,
        account_id: AccountId,
        timestamp: DateTime<Utc>,
    },
    UpstreamComplete {
        request_id: String,
        account_id: AccountId,
        status: u16,
        latency_ms: u64,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        timestamp: DateTime<Utc>,
    },
    UpstreamError {
        request_id: String,
        account_id: Option<AccountId>,
        error: String,
        timestamp: DateTime<Utc>,
    },
    Cooldown {
        account_id: AccountId,
        reason: String,
        until: DateTime<Utc>,
    },
    RefreshAttempt {
        account_id: AccountId,
        success: bool,
        timestamp: DateTime<Utc>,
    },
}

/// Process-wide event fan-out. Cheap to clone (`Sender` is `Arc`-backed).
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<RequestEvent>,
}

impl EventBus {
    /// Create a new bus with the given broadcast buffer capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish an event. If no subscribers exist, the event is silently
    /// dropped (this is the broadcast channel's documented behavior); if
    /// subscribers exist but the internal buffer is overflowed, the receiver
    /// will observe `RecvError::Lagged` and we record a backpressure drop
    /// here too — `send` itself only fails when there are zero receivers.
    pub fn publish(&self, event: RequestEvent) {
        if let Err(_e) = self.tx.send(event) {
            metrics::record_stream_backpressure_drop();
        }
    }

    /// Subscribe a new consumer. Each receiver has its own cursor into the
    /// buffer; slow consumers lag independently.
    pub fn subscribe(&self) -> broadcast::Receiver<RequestEvent> {
        self.tx.subscribe()
    }

    /// Returns the current number of active subscribers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    fn sample_event() -> RequestEvent {
        RequestEvent::Accepted {
            request_id: "req_test".to_string(),
            protocol: "anthropic".to_string(),
            model_hint: Some("claude-3-5-sonnet".to_string()),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn publish_and_subscribe_roundtrip() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        bus.publish(sample_event());
        let got = rx.recv().await.expect("recv");
        match got {
            RequestEvent::Accepted { request_id, .. } => assert_eq!(request_id, "req_test"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive_event() {
        let bus = EventBus::new(8);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.publish(sample_event());
        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[tokio::test]
    async fn lagged_receiver_observes_lag_error() {
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();
        for _ in 0..5 {
            bus.publish(sample_event());
        }
        let err = rx.recv().await.expect_err("expected lag error");
        match err {
            tokio::sync::broadcast::error::RecvError::Lagged(n) => assert!(n >= 1),
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(rx.recv().await.is_ok());
        assert!(rx.recv().await.is_ok());
        bus.publish(sample_event());
        assert!(rx.recv().await.is_ok());
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_records_backpressure_drop() {
        let bus = EventBus::new(4);
        let before = metrics::registry().stream_backpressure_drops_total.get();
        bus.publish(sample_event());
        let after = metrics::registry().stream_backpressure_drops_total.get();
        assert!(
            after > before,
            "expected drop counter to advance: {before} -> {after}"
        );
    }

    #[tokio::test]
    async fn try_recv_returns_empty_when_idle() {
        let bus = EventBus::new(4);
        let mut rx = bus.subscribe();
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }
}
