//! Compatibility surface for pure relay helpers.

pub use crate::relay_core_discovery::{
    count_claude_stop_hook_summaries, discover_recent_claude_transcript,
    discover_recent_codex_transcript, read_claude_transcript_with_retry,
};
#[cfg(test)]
pub(crate) use crate::relay_core_discovery::{
    discover_recent_claude_transcript_in_root, discover_recent_codex_transcript_in_root,
};
pub use crate::relay_core_io::{
    drop_seen_signature, log_event, pane_uses_claude, pane_uses_codex, preview,
    submit_delay_for_agent,
};
#[cfg(test)]
pub use crate::relay_core_io::{DEFAULT_CLAUDE_SUBMIT_DELAY_MS, DEFAULT_SUBMIT_DELAY_MS};
pub use crate::relay_core_prompt::{codex_transcript_contains_user_prompt, normalize_prompt_text};

#[cfg(test)]
#[path = "relay_core_tests.rs"]
mod tests;
