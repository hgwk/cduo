use crate::message::Message;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Deduplication entry: stores the expiry time for a content hash.
struct DedupEntry {
    expires_at: Instant,
}

/// A publish/subscribe message bus for routing messages between nodes.
///
/// Each subscriber (identified by a `node_id`) gets an `mpsc::Receiver<Message>`.
/// When a message is published, it is delivered to the subscriber whose `node_id`
/// matches the message's `target_node_id`. Messages are deduplicated by
/// `content_hash` within a configurable time window.
pub struct MessageBus {
    /// Map from node_id to the sending half of an mpsc channel.
    subscribers: HashMap<String, mpsc::Sender<Message>>,
    /// Deduplication cache: content_hash -> expiry time.
    dedup_cache: HashMap<String, DedupEntry>,
    /// Duration in seconds for the deduplication window.
    dedup_window_secs: u64,
}

impl MessageBus {
    /// Create a new `MessageBus` with the default dedup window of 10 seconds.
    pub fn new() -> Self {
        Self::with_dedup_window(10)
    }

    /// Create a new `MessageBus` with a custom deduplication window.
    pub fn with_dedup_window(dedup_window_secs: u64) -> Self {
        Self {
            subscribers: HashMap::new(),
            dedup_cache: HashMap::new(),
            dedup_window_secs,
        }
    }

    /// Subscribe a node to the message bus.
    ///
    /// Returns a `Receiver<Message>` that will receive all messages whose
    /// `target_node_id` matches the given `node_id`. If the node was already
    /// subscribed, the old channel is replaced with a new one.
    pub fn subscribe(&mut self, node_id: &str) -> mpsc::Receiver<Message> {
        let (tx, rx) = mpsc::channel(128);
        self.subscribers.insert(node_id.to_string(), tx);
        rx
    }

    /// Publish a message to the bus.
    ///
    /// The message is routed to the subscriber whose `node_id` matches
    /// `msg.target_node_id`. If no such subscriber exists, the message is
    /// silently discarded.
    ///
    /// Messages are deduplicated by compound key (source+target+origin+content_hash):
    /// the same content from the same source to the same target with the same origin
    /// is rejected within the dedup window. Returns `true` if the message was delivered
    /// (or would have been delivered to a subscriber), `false` if it was deduplicated.
    pub fn publish(&mut self, msg: Message) -> bool {
        self.clean_expired();

        let dedup_key = self.dedup_key(&msg);
        if self.is_duplicate(&dedup_key) {
            return false;
        }

        self.record_hash(&dedup_key);

        if let Some(tx) = self.subscribers.get(&msg.target_node_id) {
            let target = msg.target_node_id.clone();
            if tx.try_send(msg).is_err() {
                self.subscribers.remove(&target);
            }
        }

        true
    }

    // --- Private helpers ---

    fn dedup_key(&self, msg: &Message) -> String {
        format!(
            "{:?}:{}:{}:{}",
            msg.origin_kind, msg.source_node_id, msg.target_node_id, msg.content_hash
        )
    }

    fn clean_expired(&mut self) {
        let now = Instant::now();
        self.dedup_cache.retain(|_, entry| entry.expires_at > now);
    }

    fn is_duplicate(&self, hash: &str) -> bool {
        self.dedup_cache
            .get(hash)
            .is_some_and(|entry| entry.expires_at > Instant::now())
    }

    fn record_hash(&mut self, hash: &str) {
        let expires_at = Instant::now() + Duration::from_secs(self.dedup_window_secs);
        self.dedup_cache
            .insert(hash.to_string(), DedupEntry { expires_at });
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_message(target: &str, content: &str) -> Message {
        Message::new_relay("source", target, content)
    }

    #[tokio::test]
    async fn publish_to_subscriber() {
        let mut bus = MessageBus::new();
        let mut rx = bus.subscribe("b");

        let msg = make_message("b", "hello from a");
        let result = bus.publish(msg);
        assert!(result, "publish should return true for valid subscriber");

        let received = rx.recv().await.expect("should receive message");
        assert_eq!(received.content, "hello from a");
        assert_eq!(received.target_node_id, "b");
    }

    #[tokio::test]
    async fn dedup_rejects_duplicate_hash_within_window() {
        let mut bus = MessageBus::new();
        let mut rx = bus.subscribe("b");

        let msg1 = make_message("b", "duplicate content");
        let hash = msg1.content_hash.clone();
        assert!(bus.publish(msg1), "first publish should succeed");

        rx.recv().await.expect("should receive first message");

        let msg2 = make_message("b", "duplicate content");
        assert_eq!(msg2.content_hash, hash, "hashes should match");
        assert!(
            !bus.publish(msg2),
            "second publish with same hash should be rejected"
        );
    }

    #[tokio::test]
    async fn no_subscriber_no_panic() {
        let mut bus = MessageBus::new();

        let msg = make_message("nonexistent", "orphan message");
        let result = bus.publish(msg);
        assert!(result, "publish with no subscriber should return true");
    }
}
