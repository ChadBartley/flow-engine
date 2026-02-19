//! In-memory queue provider using tokio broadcast channels.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};

use crate::errors::QueueError;
use crate::traits::{MessageReceipt, QueueMessage, QueueProvider, QueueReceiver};

const BROADCAST_CAPACITY: usize = 1000;

/// In-memory queue backed by per-topic `tokio::sync::broadcast` channels.
///
/// Messages are only delivered to receivers that were subscribed at the time
/// of publication (standard broadcast semantics). `ack` is a no-op.
pub struct InMemoryQueue {
    topics: Arc<RwLock<HashMap<String, broadcast::Sender<InternalMessage>>>>,
}

#[derive(Clone)]
struct InternalMessage {
    payload: Vec<u8>,
    headers: HashMap<String, String>,
}

impl InMemoryQueue {
    /// Create a new empty in-memory queue.
    pub fn new() -> Self {
        Self {
            topics: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QueueProvider for InMemoryQueue {
    async fn publish(
        &self,
        topic: &str,
        payload: &[u8],
        headers: Option<&HashMap<String, String>>,
    ) -> Result<(), QueueError> {
        let topics = self.topics.read().await;
        if let Some(tx) = topics.get(topic) {
            let msg = InternalMessage {
                payload: payload.to_vec(),
                headers: headers.cloned().unwrap_or_default(),
            };
            // Ignore send errors — no active receivers is not an error.
            let _ = tx.send(msg);
        }
        Ok(())
    }

    async fn subscribe(&self, topic: &str) -> Result<QueueReceiver, QueueError> {
        let mut topics = self.topics.write().await;
        let broadcast_tx = topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(BROADCAST_CAPACITY).0);

        let mut broadcast_rx = broadcast_tx.subscribe();
        let (mpsc_tx, mpsc_rx) = tokio::sync::mpsc::channel(BROADCAST_CAPACITY);

        // Bridge: forward broadcast messages into the mpsc channel expected
        // by QueueReceiver.
        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(msg) => {
                        let queue_msg = QueueMessage {
                            payload: msg.payload,
                            headers: msg.headers,
                            receipt: MessageReceipt {
                                id: uuid::Uuid::new_v4().to_string(),
                            },
                        };
                        if mpsc_tx.send(queue_msg).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });

        Ok(QueueReceiver { rx: mpsc_rx })
    }

    async fn ack(&self, _receipt: &MessageReceipt) -> Result<(), QueueError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_publish_subscribe() {
        let queue = InMemoryQueue::new();
        let mut sub = queue.subscribe("test-topic").await.unwrap();
        queue.publish("test-topic", b"hello", None).await.unwrap();

        let msg = sub.rx.recv().await.unwrap();
        assert_eq!(msg.payload, b"hello");
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let queue = InMemoryQueue::new();
        let mut sub1 = queue.subscribe("multi").await.unwrap();
        let mut sub2 = queue.subscribe("multi").await.unwrap();

        queue.publish("multi", b"data", None).await.unwrap();

        let msg1 = sub1.rx.recv().await.unwrap();
        let msg2 = sub2.rx.recv().await.unwrap();
        assert_eq!(msg1.payload, b"data");
        assert_eq!(msg2.payload, b"data");
    }

    #[tokio::test]
    async fn test_ack_noop() {
        let queue = InMemoryQueue::new();
        let receipt = MessageReceipt {
            id: "test".to_string(),
        };
        queue.ack(&receipt).await.unwrap();
    }
}
