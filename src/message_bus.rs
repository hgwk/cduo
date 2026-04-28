use crate::message::Message;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Deduplication entry: stores the expiry time for a content hash.
struct DedupEntry {
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Delivered,
    NoSubscriber,
    Deduplicated,
    SubscriberClosed,
}

impl PublishResult {
    pub fn is_delivered(self) -> bool {
        matches!(self, Self::Delivered)
    }

    pub fn log_label(self) -> &'static str {
        match self {
            Self::Delivered => "publish",
            Self::NoSubscriber => "no_subscriber",
            Self::Deduplicated => "dedup",
            Self::SubscriberClosed => "subscriber_closed",
        }
    }
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
    /// `msg.target_node_id`. The return value distinguishes successful delivery
    /// from no-subscriber, deduplication, and closed/full subscriber cases.
    pub fn publish(&mut self, msg: Message) -> PublishResult {
        self.clean_expired();

        let dedup_key = self.dedup_key(&msg);
        if self.is_duplicate(&dedup_key) {
            return PublishResult::Deduplicated;
        }

        let target = msg.target_node_id.clone();
        let Some(tx) = self.subscribers.get(&target) else {
            self.record_hash(&dedup_key);
            return PublishResult::NoSubscriber;
        };

        match tx.try_send(msg) {
            Ok(()) => {
                self.record_hash(&dedup_key);
                PublishResult::Delivered
            }
            Err(_) => {
                self.subscribers.remove(&target);
                PublishResult::SubscriberClosed
            }
        }
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
        assert_eq!(
            result,
            PublishResult::Delivered,
            "publish should report delivery for valid subscriber"
        );

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
        assert_eq!(
            bus.publish(msg1),
            PublishResult::Delivered,
            "first publish should succeed"
        );

        rx.recv().await.expect("should receive first message");

        let msg2 = make_message("b", "duplicate content");
        assert_eq!(msg2.content_hash, hash, "hashes should match");
        assert_eq!(
            bus.publish(msg2),
            PublishResult::Deduplicated,
            "second publish with same hash should be rejected"
        );
    }

    #[tokio::test]
    async fn no_subscriber_is_reported() {
        let mut bus = MessageBus::new();

        let msg = make_message("nonexistent", "orphan message");
        let result = bus.publish(msg);
        assert_eq!(result, PublishResult::NoSubscriber);
    }

    #[tokio::test]
    async fn closed_subscriber_is_reported() {
        let mut bus = MessageBus::new();
        let rx = bus.subscribe("b");
        drop(rx);

        let result = bus.publish(make_message("b", "closed target"));
        assert_eq!(result, PublishResult::SubscriberClosed);
    }
}
