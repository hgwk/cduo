use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use regex::Regex;

#[derive(Debug, Clone)]
pub struct Message {
    pub source: String,
    #[allow(dead_code)]
    pub target: String,
    pub content: String,
    pub signature: String,
    #[allow(dead_code)]
    pub ready_at: Instant,
}

pub struct RelayEngine {
    pub max_turns: u32,
    pub cooldown_ms: u64,
    pub queues: HashMap<String, VecDeque<Message>>,
    pub last_forwarded: HashMap<String, String>,
    pub turns: u32,
    pub last_send: HashMap<String, Instant>,
    #[allow(dead_code)]
    pub auto_pipeline: bool,
    seen_signatures: HashSet<String>,
}

fn prompt_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"(?m)^›(?:\s.*)?$").unwrap(),
        Regex::new(r"(?m)^❯\s*$").unwrap(),
        Regex::new(r"(?m)^─+$").unwrap(),
        Regex::new(r"(?m)^[#$%]\s+").unwrap(),
        Regex::new(r"(?m)^[^ \t\r\n]+@[^ \t\r\n]+(?::[^ \t\r\n]+)?[$#]\s+").unwrap(),
        Regex::new(r"(?m)\(esc to interrupt\)$").unwrap(),
        Regex::new(r"(?m)\? for shortcuts$").unwrap(),
    ]
}

impl RelayEngine {
    pub fn new() -> Self {
        Self {
            max_turns: 10,
            cooldown_ms: 3000,
            queues: HashMap::new(),
            last_forwarded: HashMap::new(),
            turns: 0,
            last_send: HashMap::new(),
            auto_pipeline: true,
            seen_signatures: HashSet::new(),
        }
    }

    pub fn queue(&mut self, target: &str, message: Message) {
        let queue = self.queues.entry(target.to_string()).or_default();

        if let Some(pos) = queue.iter().position(|m| m.source == message.source) {
            queue[pos] = message;
        } else {
            queue.push_back(message);
        }
    }

    pub fn process(&mut self, target: &str) -> Result<Option<Message>> {
        if self.turns >= self.max_turns {
            return Ok(None);
        }

        if let Some(last) = self.last_send.get(target) {
            let elapsed = last.elapsed().as_millis() as u64;
            if elapsed < self.cooldown_ms {
                return Ok(None);
            }
        }

        let queue = self.queues.entry(target.to_string()).or_default();

        while let Some(msg) = queue.front() {
            if self.seen_signatures.contains(&msg.signature) {
                queue.pop_front();
                continue;
            }

            if msg.content.is_empty() {
                queue.pop_front();
                continue;
            }

            break;
        }

        let msg = match queue.front() {
            Some(m) => m.clone(),
            None => return Ok(None),
        };

        queue.pop_front();
        self.seen_signatures.insert(msg.signature.clone());
        self.last_forwarded.insert(target.to_string(), msg.signature.clone());
        self.last_send.insert(target.to_string(), Instant::now());
        self.turns += 1;

        Ok(Some(msg))
    }

    pub fn is_pane_ready(&self, buffer: &str) -> bool {
        let patterns = prompt_patterns();
        patterns.iter().any(|p| p.is_match(buffer))
    }

    #[allow(dead_code)]
    pub fn reset_turns(&mut self) {
        self.turns = 0;
    }

    #[allow(dead_code)]
    pub fn has_capacity(&self) -> bool {
        self.turns < self.max_turns
    }

    #[allow(dead_code)]
    pub fn cooldown_remaining(&self, target: &str) -> Duration {
        match self.last_send.get(target) {
            Some(last) => {
                let elapsed = last.elapsed().as_millis() as u64;
                if elapsed < self.cooldown_ms {
                    Duration::from_millis(self.cooldown_ms - elapsed)
                } else {
                    Duration::ZERO
                }
            }
            None => Duration::ZERO,
        }
    }

    #[allow(dead_code)]
    pub fn queue_len(&self, target: &str) -> usize {
        self.queues.get(target).map_or(0, |q| q.len())
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.queues.clear();
        self.seen_signatures.clear();
    }

    #[allow(dead_code)]
    pub fn clear_target(&mut self, target: &str) {
        self.queues.remove(target);
    }
}

impl Default for RelayEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(source: &str, target: &str, content: &str, sig: &str) -> Message {
        Message {
            source: source.to_string(),
            target: target.to_string(),
            content: content.to_string(),
            signature: sig.to_string(),
            ready_at: Instant::now(),
        }
    }

    #[test]
    fn queues_and_processes_message() {
        let mut engine = RelayEngine::new();
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        let msg = engine.process("pane-a").unwrap().unwrap();
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.signature, "sig1");
    }

    #[test]
    fn upsert_replaces_same_source() {
        let mut engine = RelayEngine::new();
        engine.queue("pane-a", make_msg("claude", "pane-a", "old", "sig1"));
        engine.queue("pane-a", make_msg("claude", "pane-a", "new", "sig2"));
        assert_eq!(engine.queue_len("pane-a"), 1);
        let msg = engine.process("pane-a").unwrap().unwrap();
        assert_eq!(msg.content, "new");
    }

    #[test]
    fn dedup_by_signature() {
        let mut engine = RelayEngine::new();
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        engine.process("pane-a").unwrap();
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        let msg = engine.process("pane-a").unwrap();
        assert!(msg.is_none());
    }

    #[test]
    fn respects_turn_limit() {
        let mut engine = RelayEngine::new();
        engine.max_turns = 2;
        engine.queue("pane-a", make_msg("claude", "pane-a", "msg1", "sig1"));
        engine.queue("pane-a", make_msg("claude", "pane-a", "msg2", "sig2"));
        engine.queue("pane-a", make_msg("claude", "pane-a", "msg3", "sig3"));

        engine.process("pane-a").unwrap();
        engine.process("pane-a").unwrap();
        let third = engine.process("pane-a").unwrap();
        assert!(third.is_none());
    }

    #[test]
    fn respects_cooldown() {
        let mut engine = RelayEngine::new();
        engine.cooldown_ms = 10000;
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        engine.process("pane-a").unwrap();
        engine.queue("pane-a", make_msg("claude", "pane-a", "world", "sig2"));
        let msg = engine.process("pane-a").unwrap();
        assert!(msg.is_none());
    }

    #[test]
    fn skips_empty_content() {
        let mut engine = RelayEngine::new();
        engine.queue("pane-a", make_msg("claude", "pane-a", "", "sig1"));
        engine.queue("pane-a", make_msg("claude", "pane-a", "real", "sig2"));
        let msg = engine.process("pane-a").unwrap().unwrap();
        assert_eq!(msg.content, "real");
    }

    #[test]
    fn detects_readiness_with_prompt() {
        let engine = RelayEngine::new();
        assert!(engine.is_pane_ready("› "));
        assert!(engine.is_pane_ready("❯ "));
        assert!(engine.is_pane_ready("$ ls\n"));
        assert!(engine.is_pane_ready("user@host:~$ "));
    }

    #[test]
    fn detects_unreadiness() {
        let engine = RelayEngine::new();
        assert!(!engine.is_pane_ready("Working on it..."));
        assert!(!engine.is_pane_ready("Thinking..."));
    }

    #[test]
    fn cooldown_remaining() {
        let mut engine = RelayEngine::new();
        engine.cooldown_ms = 5000;
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        engine.process("pane-a").unwrap();
        let remaining = engine.cooldown_remaining("pane-a");
        assert!(remaining.as_millis() > 0);
        assert!(remaining.as_millis() <= 5000);
    }

    #[test]
    fn clear_removes_all() {
        let mut engine = RelayEngine::new();
        engine.queue("pane-a", make_msg("claude", "pane-a", "hello", "sig1"));
        engine.queue("pane-b", make_msg("codex", "pane-b", "world", "sig2"));
        engine.clear();
        assert_eq!(engine.queue_len("pane-a"), 0);
        assert_eq!(engine.queue_len("pane-b"), 0);
    }

    #[test]
    fn reset_turns_allows_more() {
        let mut engine = RelayEngine::new();
        engine.max_turns = 1;
        engine.cooldown_ms = 0;
        engine.queue("pane-a", make_msg("claude", "pane-a", "msg1", "sig1"));
        engine.process("pane-a").unwrap();
        engine.queue("pane-a", make_msg("codex", "pane-a", "msg2", "sig2"));
        engine.reset_turns();
        let msg = engine.process("pane-a").unwrap().unwrap();
        assert_eq!(msg.content, "msg2");
    }

    #[test]
    fn has_capacity() {
        let mut engine = RelayEngine::new();
        engine.max_turns = 1;
        assert!(engine.has_capacity());
        engine.queue("pane-a", make_msg("claude", "pane-a", "msg", "sig1"));
        engine.process("pane-a").unwrap();
        assert!(!engine.has_capacity());
    }

    #[test]
    fn pipeline_claude_extractor_to_relay() {
        use crate::extractors;

        let pane_a_buffer = "⏺ hello from pane A\n$ ";
        let extracted = extractors::claude::extract(pane_a_buffer);
        assert_eq!(extracted.output, "hello from pane A $");
        assert!(extracted.signature.is_some());

        let mut engine = RelayEngine::new();
        assert!(engine.is_pane_ready(pane_a_buffer));

        let msg = Message {
            source: "a".to_string(),
            target: "b".to_string(),
            content: extracted.output,
            signature: extracted.signature.unwrap(),
            ready_at: Instant::now(),
        };
        engine.queue("b", msg);

        let forwarded = engine.process("b").unwrap();
        assert!(forwarded.is_some());
        assert_eq!(forwarded.unwrap().content, "hello from pane A $");
    }

    #[test]
    fn pipeline_codex_extractor_to_relay() {
        use crate::extractors;

        let pane_a_buffer = "some output here\n$ ";
        let extracted = extractors::codex::extract(pane_a_buffer);
        assert!(!extracted.output.is_empty());

        let mut engine = RelayEngine::new();
        assert!(engine.is_pane_ready(pane_a_buffer));

        let msg = Message {
            source: "a".to_string(),
            target: "b".to_string(),
            content: extracted.output.clone(),
            signature: extracted.signature.unwrap_or_default(),
            ready_at: Instant::now(),
        };
        engine.queue("b", msg);

        let forwarded = engine.process("b").unwrap();
        assert!(forwarded.is_some());
    }
}
