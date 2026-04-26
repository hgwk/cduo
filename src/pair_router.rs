use crate::message::{Message, OriginKind};

/// A 1:1 message router that implements the relay policy for cduo's two-node system.
///
/// Routing rules:
/// - Agent response → relay to counterpart
/// - Relay message → do not route again
pub struct PairRouter {
    node_a: String,
    node_b: String,
}

impl PairRouter {
    /// Initialize the router with the two node IDs in the pair.
    pub fn new(node_a: &str, node_b: &str) -> Self {
        Self {
            node_a: node_a.to_string(),
            node_b: node_b.to_string(),
        }
    }

    /// Returns the other node in the pair, or None if the given node_id is not part of this pair.
    pub fn counterpart(&self, node_id: &str) -> Option<&str> {
        if node_id == self.node_a {
            Some(&self.node_b)
        } else if node_id == self.node_b {
            Some(&self.node_a)
        } else {
            None
        }
    }

    /// Core routing logic. Returns a relay message if the input should be forwarded, None otherwise.
    ///
    /// - OriginKind::Agent → creates a new relay message from source to counterpart
    /// - OriginKind::Relay → None
    pub fn route(&self, msg: &Message) -> Option<Message> {
        match msg.origin_kind {
            OriginKind::Agent => {
                let counterpart = self.counterpart(&msg.source_node_id)?;
                Some(Message::new_relay(
                    &msg.source_node_id,
                    counterpart,
                    &msg.content,
                ))
            }
            OriginKind::Relay => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

    fn router() -> PairRouter {
        PairRouter::new("node-a", "node-b")
    }

    #[test]
    fn agent_routes_to_counterpart() {
        let r = router();
        let msg = Message::new_agent("node-a", "agent response from A");
        let relay = r.route(&msg).expect("Agent message should route");
        assert_eq!(relay.source_node_id, "node-a");
        assert_eq!(relay.target_node_id, "node-b");
        assert!(matches!(relay.origin_kind, OriginKind::Relay));
        assert!(matches!(relay.role, Role::User));
        assert_eq!(relay.content, "agent response from A");
    }

    #[test]
    fn relay_does_not_route() {
        let r = router();
        let msg = Message::new_relay("node-a", "node-b", "relayed content");
        assert!(r.route(&msg).is_none());
    }

    #[test]
    fn unknown_node_returns_none() {
        let r = router();
        let msg = Message::new_agent("unknown-node", "some content");
        assert!(r.route(&msg).is_none());
        assert!(r.counterpart("unknown-node").is_none());
    }

    #[test]
    fn counterpart_works() {
        let r = router();
        assert_eq!(r.counterpart("node-a"), Some("node-b"));
        assert_eq!(r.counterpart("node-b"), Some("node-a"));
        assert!(r.counterpart("node-c").is_none());
    }
}
