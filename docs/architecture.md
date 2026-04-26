# cduo Architecture

`cduo` is a single Rust binary that runs two Claude or Codex agent processes in direct PTYs and shows them through a split tmux workspace. tmux is UI only; relay control is handled by the daemon.

## Runtime Shape

```text
cduo CLI
  └─ cduo daemon
       ├─ PTY A ── claude/codex
       ├─ PTY B ── claude/codex
       ├─ Claude hook server
       ├─ Unix control socket
       └─ MessageBus + PairRouter

tmux
  ├─ pane A: cduo __attach-pane <session> a
  └─ pane B: cduo __attach-pane <session> b
```

## Relay Sources

- Claude: `Stop` hook posts `terminal_id` and `transcript_path` to the local hook server.
- Codex: the daemon discovers recent Codex rollout JSONL files under `~/.codex/sessions/` whose `session_meta.payload.cwd` matches the workspace.
- PTY output is a wake-up signal and user-visible UI stream only. It is not parsed as message content.

## Relay Flow

1. An agent produces a response.
2. The daemon reads the latest assistant text from that agent's transcript JSONL.
3. `Message::new_agent` creates an agent message.
4. `PairRouter` maps source pane `a` to `b`, or `b` to `a`.
5. `MessageBus` suppresses duplicate source/target/content deliveries.
6. The relay writes the message text to the target PTY and then sends Enter.

## Core Modules

- `src/daemon.rs`: session lifecycle, PTY tasks, control socket, relay loop.
- `src/pty.rs`: `portable-pty` process spawning and stdin/stdout bridge.
- `src/hook.rs`: Claude `Stop` hook HTTP endpoint.
- `src/transcripts/claude.rs`: latest assistant text from Claude transcript JSONL.
- `src/transcripts/codex.rs`: latest assistant `output_text` from Codex rollout JSONL.
- `src/message.rs`: relay message model.
- `src/message_bus.rs`: pub/sub delivery with deduplication.
- `src/pair_router.rs`: current 1:1 routing policy.
- `src/session.rs`: persisted session metadata.
- `src/tmux.rs`: tmux workspace creation and attach helpers.

## Non-Goals In Current 1:1 Mode

- No terminal screen scraping for relay content.
- No regex cleanup of TUI output.
- No readiness gate based on prompt matching.
- No ratatui client.
- No N:N graph routing.

Those can be added later, but the stable base is transcript-sourced 1:1 relay.
