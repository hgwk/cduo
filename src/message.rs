use rand::Rng;
use sha2::{Digest, Sha256};
use std::fmt;

/// The origin of a message in the cduo 1:1 message bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginKind {
    /// Message originated from an AI agent.
    Agent,
    /// Message was relayed from one node to another.
    Relay,
}

/// The role of a message sender.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// The sender is acting as a user.
    User,
    /// The sender is acting as an assistant.
    Assistant,
}

/// A core message in the cduo 1:1 message bus.
#[derive(Clone, Debug)]
pub struct Message {
    /// Unique identifier for the message.
    pub id: String,
    /// The node that sent this message (e.g. "a" or "b").
    pub source_node_id: String,
    /// The node that should receive this message (e.g. "a" or "b").
    pub target_node_id: String,
    /// The kind of origin for this message.
    pub origin_kind: OriginKind,
    /// The role of the message sender.
    pub role: Role,
    /// The textual content of the message.
    pub content: String,
    /// SHA-256 hash of the content.
    pub content_hash: String,
}

impl Message {
    /// Generate a UUID-like message ID.
    fn generate_id() -> String {
        let mut rng = rand::thread_rng();
        let id: u64 = rng.gen();
        format!("msg-{:016x}", id)
    }

    /// Compute the SHA-256 hash of the given content as a hex string.
    fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    /// Create a new agent-originated message.
    pub fn new_agent(source: &str, content: &str) -> Self {
        Self {
            id: Self::generate_id(),
            source_node_id: source.to_string(),
            target_node_id: String::new(),
            origin_kind: OriginKind::Agent,
            role: Role::Assistant,
            content: content.to_string(),
            content_hash: Self::compute_hash(content),
        }
    }

    /// Create a new relay message between two nodes.
    pub fn new_relay(source: &str, target: &str, content: &str) -> Self {
        Self {
            id: Self::generate_id(),
            source_node_id: source.to_string(),
            target_node_id: target.to_string(),
            origin_kind: OriginKind::Relay,
            role: Role::User,
            content: content.to_string(),
            content_hash: Self::compute_hash(content),
        }
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Message[{}: {} -> {}] ({:?}/{:?}): {}",
            self.id,
            self.source_node_id,
            self.target_node_id,
            self.origin_kind,
            self.role,
            self.content.chars().take(50).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_agent() {
        let msg = Message::new_agent("b", "agent response");
        assert_eq!(msg.source_node_id, "b");
        assert!(matches!(msg.origin_kind, OriginKind::Agent));
        assert!(matches!(msg.role, Role::Assistant));
        assert!(!msg.content_hash.is_empty());
    }

    #[test]
    fn test_new_relay() {
        let msg = Message::new_relay("a", "b", "relayed content");
        assert_eq!(msg.source_node_id, "a");
        assert_eq!(msg.target_node_id, "b");
        assert!(matches!(msg.origin_kind, OriginKind::Relay));
    }

    #[test]
    fn test_content_hash_consistency() {
        let content = "test content";
        let msg1 = Message::new_agent("a", content);
        let msg2 = Message::new_agent("b", content);
        assert_eq!(msg1.content_hash, msg2.content_hash);
    }

    #[test]
    fn test_content_hash_uniqueness() {
        let msg1 = Message::new_agent("a", "content one");
        let msg2 = Message::new_agent("a", "content two");
        assert_ne!(msg1.content_hash, msg2.content_hash);
    }

    #[test]
    fn test_message_display() {
        let msg = Message::new_agent("a", "hello");
        let display = format!("{}", msg);
        assert!(display.contains("msg-"));
        assert!(display.contains("a ->"));
        assert!(display.contains("hello"));
    }
}
