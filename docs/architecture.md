# cduo Architecture

`cduo` is a single Rust binary that runs two Claude or Codex agent processes in direct PTYs and renders them in a native ratatui split-pane TUI. The running `cduo` process *is* the session — there is no background daemon, no control socket, and no tmux dependency.

Every `start`, `claude`, or `codex` invocation creates a fresh foreground
session. `--new` is currently accepted for CLI compatibility but does not change
native runtime behavior.

## Runtime Shape

```text
cduo (foreground process)
  ├─ ratatui UI loop (blocking thread)
  │    ├─ Pane A: PTY ── claude/codex   ── vt100 parser ── ratatui buffer
  │    └─ Pane B: PTY ── claude/codex   ── vt100 parser ── ratatui buffer
  └─ tokio runtime
       ├─ Claude hook HTTP server (auto-picked port 53333+)
       ├─ Relay task (transcript polling + hook events)
       └─ MessageBus + PairRouter
```

The UI thread owns the PTY writers. The relay task communicates with the UI via three mpsc channels:

- `hook_rx` — Claude `Stop` hook events arrive here.
- `input_tx` — UI forwards each completed (pane, line) submission to the relay so codex rollouts can be matched to the owning pane.
- `write_tx` — Relay sends `(target_pane, bytes)` tuples; the UI loop drains them and writes to the target PTY (bracketed-paste body, then a delayed `\r`). If relay delivery is paused with `Ctrl-P`, the UI queues these writes and flushes them in order when resumed.

Session logs are written under the platform state directory:

- macOS: `~/Library/Application Support/works.higgs.cduo/native/`
- Linux: `~/.local/state/works.higgs.cduo/native/`

Runtime controls stay local to the foreground UI:

- `Ctrl-R`: manually relay the current pane to its peer.
- `Ctrl-X`: clear queued relay writes while relay delivery is paused.
- `Ctrl-1`: toggle A -> B relay delivery.
- `Ctrl-2`: toggle B -> A relay delivery.
- `Ctrl-G`: show recent relay log/status.
- `Ctrl-Z`: cycle layout preset/maximize mode.
- `Ctrl-L`: toggle rows/columns.
- `Ctrl-P`: pause or resume automatic relay delivery.

`CDUO_RELAY_PREFIX` prepends a short instruction to relayed messages before
they are published or manually sent.
`CDUO_MAX_RELAY_TURNS` stops automatic relay after N publishes, and
`CDUO_STOP_RELAY` or `[CDUO_STOP]` in an agent answer stops automatic relay.

## Relay Sources

- Claude: `Stop` hook posts `terminal_id` and `transcript_path` to the local hook server.
- Codex: the relay discovers recent Codex rollout JSONL files under `~/.codex/sessions/` whose `session_meta.payload.cwd` matches the workspace and whose user prompts match a pending submission from that pane.
- PTY output is rendered for the user but is **not** parsed as message content; transcripts are the only source of relay text.

`cduo init` installs the Claude `Stop` hook into the project. Native `start`
commands do not edit project files.

## Relay Flow

1. An agent produces a response.
2. The relay reads the latest assistant text from that agent's transcript JSONL (Claude on hook event; codex on a 250 ms poll).
3. `Message::new_agent` creates an agent message.
4. `PairRouter` maps source pane `a` to `b`, or `b` to `a`.
5. `MessageBus` suppresses duplicate source/target/content deliveries.
6. The relay sends the bracketed-paste payload over `write_tx`, sleeps a per-agent submit delay, then sends `\r`.

## Core Modules

- `src/main.rs`: CLI dispatch; spawns the native runtime in the foreground.
- `src/cli.rs`: clap definitions for `start` / `claude` / `codex` / `status` / etc.
- `src/native/runtime.rs`: ratatui UI loop, PTY ownership, raw-mode/alt-screen guard, hook server bootstrap, channel wiring.
- `src/native/pane.rs`: per-pane PTY spawn + reader thread + vt100 parser.
- `src/native/input.rs`: crossterm `KeyEvent` → PTY bytes; global runtime-control classification.
- `src/native/ui.rs`: vt100 `Screen` → ratatui `Buffer` rendering with color/attribute mapping.
- `src/native/relay.rs`: `tokio::select!` loop over hook / input / 250 ms tick; pane↔transcript binding; bracketed-paste delivery.
- `src/relay_core.rs`: pure helpers shared by the relay loop — transcript parsing, codex rollout discovery, dedup, structured logging.
- `src/hook.rs`: Claude `Stop` hook HTTP endpoint.
- `src/transcripts/claude.rs`: latest assistant text from Claude transcript JSONL.
- `src/transcripts/codex.rs`: latest assistant `output_text` from Codex rollout JSONL.
- `src/message.rs` / `src/message_bus.rs` / `src/pair_router.rs`: in-process pub/sub with dedup and 1:1 routing.
- `src/session.rs`: state-directory helpers (per-session log file path).

## Non-Goals In Current 1:1 Mode

- No terminal screen scraping for relay content.
- No regex cleanup of TUI output.
- No readiness gate based on prompt matching.
- No background daemon, control socket, or attach mechanism.
- No tmux dependency.
- No N:N graph routing.
- No project-file writes from native `start` commands.

Those can be added later, but the stable base is transcript-sourced 1:1 relay running inside a single foreground process.

See [`graph-routing-roadmap.md`](graph-routing-roadmap.md) for the planned
`1:N` and `N:N` routing extension.
