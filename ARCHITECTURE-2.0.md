# cduo Architecture 2.0 — Final Proposal

> Based on review by Oracle (architecture) and Momus (planning).

## Executive Summary

**Migrate cduo to a pure Rust binary.** Remove the Python pty-host, keep tmux as the default UI (zero UI work), and optionally add a ratatui client later. Ship via npm as a platform-specific binary wrapper (esbuild/turbo pattern).

The Node.js + Rust split in the draft added process boundaries instead of removing them. A single Rust binary eliminates IPC, reduces failure modes, and still distributes through npm.

---

## Guiding Principles

1. **Single binary core** — One process does everything: PTY, relay, hook server, CLI
2. **Event-driven relay** — PTY output → tokio channel → relay engine → target PTY. No polling
3. **tmux as default UI** — It already solves scrollback, copy-paste, resize, detach/reattach
4. **Client/daemon separation** — Daemon outlives the client (tmux attach-style reattach)
5. **Backward compatibility** — `cduo resume`, `cduo doctor`, `cduo init` work exactly the same

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  cduo (Rust binary)                                     │
│                                                         │
│  ┌─────────────┐    ┌─────────────┐    ┌───────────┐   │
│  │ CLI (clap)  │───►│   Daemon    │◄───│   TUI     │   │
│  │             │    │   (tokio)   │    │ (ratatui) │   │
│  └─────────────┘    └──────┬──────┘    │  optional │   │
│                            │           └───────────┘   │
│                    ┌───────┴───────┐                   │
│                    │  Relay Engine │                   │
│                    │   (channels)  │                   │
│                    └───────┬───────┘                   │
│              ┌─────────────┼─────────────┐             │
│              ▼             ▼             ▼             │
│         ┌────────┐   ┌─────────┐   ┌──────────┐       │
│         │PTY (A) │   │HTTP Hook│   │PTY (B)   │       │
│         └───┬────┘   │ Server  │   └────┬─────┘       │
│             │        └─────────┘        │             │
│             ▼                           ▼             │
│         claude/codex                claude/codex      │
│                                                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │  tmux (external, layout-only, default)          │  │
│  │  ┌──────────────┬──────────────────────────┐    │  │
│  │  │  pane A      │      pane B              │    │  │
│  │  │  (attach to  │  (attach to              │    │  │
│  │  │   PTY A)     │   PTY B)                 │    │  │
│  │  └──────────────┴──────────────────────────┘    │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### Key Design Decisions

| Decision | Before (v1) | After (v2) | Rationale |
|----------|-------------|------------|-----------|
| **Runtime** | Node.js + Python | Rust (tokio) | Single binary, memory safety, async I/O |
| **PTY** | Python `pty-host.py` | `portable-pty` crate | Cross-platform, no Python dependency |
| **UI** | tmux only | tmux default + optional ratatui client | tmux solves scrollback/copy/reattach for free |
| **Relay** | JS intervals (500ms/1500ms) | tokio channel events | True event-driven, lower latency |
| **Distribution** | `npm install -g` pure JS | npm binary wrapper | Same UX, ships platform-specific binary |
| **Process count** | 4+ (Node, Python×2, tmux) | 3 (cduo daemon, agent×2, tmux) | Fewer boundaries, fewer failure modes |

---

## Component Breakdown

### 1. CLI (`src/cli.rs`)
- `clap` for argument parsing
- Commands: `start`, `stop`, `status`, `resume`, `doctor`, `init`, `backup`, `uninstall`, `update`, `version`
- Thin orchestration: spawn daemon, connect to daemon socket, print output
- Session metadata read/write (serde + JSON)

### 2. Daemon (`src/daemon.rs`)
- **tokio runtime** with the following tasks:
  - **PTY task** ×2: blocking `read()`/`write()` on PTY fds wrapped in `tokio::task::spawn_blocking`, communicating via `tokio::sync::mpsc`
  - **Relay task**: receives output from PTY tasks, runs extraction/dedup/queue, writes to target PTY
  - **Hook server task**: `hyper` or `axum` HTTP server on `127.0.0.1:PORT`
  - **Control socket task**: Unix socket accepting JSON-RPC from CLI clients
- **Session state**: in-memory + periodic disk persistence

### 3. PTY Manager (`src/pty.rs`)
- Uses `portable-pty` crate
- Spawns child process with env vars (`TERMINAL_ID`, `ORCHESTRATION_PORT`)
- Handles `SIGWINCH` → `ioctl(TIOCSWINSZ)` on PTY master
- Detects child exit via `tokio::signal::unix::SignalKind::Child` + `waitpid`

### 4. Relay Engine (`src/relay.rs`)
- **Event-driven** (no `on_tick()` polling)
- **Input**: PTY output chunks, hook events
- **Output**: Messages to target PTY stdin
- **Queue per pane**: `VecDeque<Message>` with upsert semantics (replace stale from same source)
- **Readiness gate**: Prompt detection (same regex patterns as v1)
- **Limits**: `max_turns`, `cooldown_ms`

### 5. Output Extractors (`src/extractors/`)
- Port of `lib/output-extractors.js` to Rust
- `claude.rs`: Hook-based (primary) + buffer scan fallback
- `codex.rs`: Stream-based with prompt-boundary detection
- `strip_ansi()` with regex (or `vte` crate for proper parsing if TUI mode)

### 6. Session Store (`src/session.rs`)
- Same paths as v1: `~/.local/state/cduo/sessions/` (Linux), `~/Library/Application Support/cduo` (macOS)
- Atomic writes: temp file → `fs::rename`
- Session metadata + runtime state

### 7. TUI Client (optional, `src/tui.rs`)
- `ratatui` + `crossterm`
- Connects to daemon via control socket
- Shows split view + status bar
- **Not the default** — tmux is default

---

## Communication Flow

### Start a session
```
User: cduo claude
CLI:  1. Find available port (53333+)
      2. Write session metadata
      3. Spawn daemon: cduo daemon --session <id>
      4. tmux new-session + split-window + attach

Daemon: 1. Open 2 PTYs
        2. Start hook server
        3. Open control socket
        4. Wait for tmux attach connections
```

### Relay flow (event-driven)
```
PTY A stdout ──► tokio mpsc ──► Relay Engine ──► dedup/signature check
                                     │
                                     ▼ (target ready)
                              PTY B stdin ◄── tokio mpsc
```

### Reattach
```
User: cduo resume
CLI:  1. Read session metadata
      2. Check daemon PID (pid file)
      3. If daemon alive: tmux attach-session
      4. If daemon dead: error, suggest restart
```

---

## Data Structures

```rust
// Core session
struct Session {
    id: String,
    name: String,
    panes: [Pane; 2],
    relay: RelayEngine,
    hook_port: u16,
    control_socket: PathBuf,
    started_at: Instant,
}

// Per-pane state
struct Pane {
    id: PaneId, // 'a' or 'b'
    pty: Box<dyn portable_pty::MasterPty>,
    child: Box<dyn portable_pty::Child>,
    buffer: CircularBuffer,
    state: PaneState,
    ready: bool,
}

enum PaneState {
    Starting,
    Idle,
    Processing,
    OutputSettled,
    ReadyForRelay,
    Cooldown { until: Instant },
    Exited { code: Option<i32> },
}

// Relay
struct RelayEngine {
    config: RelayConfig,
    queue: HashMap<PaneId, VecDeque<Message>>,
    last_forwarded: HashMap<PaneId, Signature>,
    turns: u32,
    last_send: HashMap<PaneId, Instant>,
}

struct Message {
    source: PaneId,
    target: PaneId,
    content: String,
    signature: Signature,
    ready_at: Instant,
}

// Control socket protocol (JSON-RPC-like)
#[derive(Serialize, Deserialize)]
enum Request {
    StartSession { spec: LaunchSpec },
    StopSession { id: String },
    GetStatus { id: String },
    ListSessions,
    AttachPane { session_id: String, pane_id: PaneId },
}
```

---

## Migration Plan (8 Phases)

### Phase 0: Foundation
- [ ] Rust project scaffolding (`cargo new`, workspace layout)
- [ ] CI: GitHub Actions for macOS (x86_64, arm64) + Linux (x86_64, aarch64)
- [ ] npm wrapper package structure (binary download script)
- [ ] Control socket protocol spec (JSON-RPC methods documented)
- **Acceptance**: CI builds for all 4 platforms. `cargo build` passes.

### Phase 1a: PTY Manager
- [ ] Integrate `portable-pty` crate
- [ ] Spawn agent process in PTY with env vars
- [ ] Read PTY stdout (blocking task + mpsc)
- [ ] Write to PTY stdin
- [ ] Handle SIGWINCH propagation
- [ ] Child exit detection
- **Acceptance**: Integration test spawns `cat`, writes input, reads output back.

### Phase 1b: Relay Engine
- [ ] Port output extractors (Claude hook + Codex stream)
- [ ] Signature-based dedup
- [ ] Per-pane queue with upsert
- [ ] Readiness gate (prompt detection)
- [ ] Turn limits + cooldown
- **Acceptance**: Unit tests match v1 JS extractor outputs byte-for-byte.

### Phase 1c: Session Lifecycle
- [ ] Session metadata store (cross-platform paths)
- [ ] Daemon spawn (PID file, control socket)
- [ ] `cduo start` command
- [ ] `cduo stop` command
- **Acceptance**: Can start a session, see it in `~/.local/state/cduo/`, stop it.

### Phase 2a: tmux Integration
- [ ] `tmux new-session`, `split-window`, `attach-session`
- [ ] tmux pane commands (attach to PTY via control socket)
- [ ] Session health check (`tmux has-session`)
- [ ] `cduo resume` with session selection logic
- **Acceptance**: `cduo start` opens tmux split. `cduo resume` reconnects.

### Phase 2b: Hook Server + Extractors
- [ ] HTTP hook server (`/hook` endpoint)
- [ ] Claude Stop hook integration
- [ ] Full output extractor parity with v1
- [ ] `cduo status` command
- **Acceptance**: Claude `Stop` hook triggers relay. Output matches v1 behavior.

### Phase 2c: Project Commands
- [ ] `cduo init` (Claude settings + CLAUDE.md)
- [ ] `cduo backup`
- [ ] `cduo uninstall`
- [ ] `cduo doctor` (checks Rust binary, tmux, agents)
- **Acceptance**: All project commands produce identical output to v1.

### Phase 3: Polish & Distribution
- [ ] npm package with binary wrapper (`postinstall` downloads platform binary)
- [ ] `cduo update` (self-updater or npm update)
- [ ] Debug socket (`cduo status --verbose`)
- [ ] Backpressure tuning (configurable buffer limits)
- [ ] Metrics (optional Prometheus endpoint)
- [ ] ratatui TUI client (optional, `--tui` flag)
- **Acceptance**: `npm install -g @hgwk/cduo` works on macOS and Linux.

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| **Rust TUI doesn't work out** | tmux is the default. TUI is Phase 3, optional. |
| **portable-pty has bugs** | Keep tmux path. PTY is internal — users don't see it. |
| **Binary distribution fails** | Provide `cargo install cduo` fallback. npm is convenience. |
| **Existing sessions break on update** | v2 reads v1 session store format (JSON is compatible). |
| **Performance regression** | Benchmark: relay latency, memory usage vs v1 before release. |
| **Build complexity** | CI handles cross-compilation. Local dev = `cargo run`. |

---

## Open Questions (Resolved)

| Question | Resolution | Phase |
|----------|-----------|-------|
| Session reattach | Daemon persists with PID file + control socket. CLI reconnects. | 2a |
| Windows support | `portable-pty` supports Windows ConPTY. Official support in v2.1. | Post-v2.0 |
| SIGWINCH | Catch SIGWINCH, `ioctl(TIOCSWINSZ)` on both PTY masters. | 1a |
| Scrollback | tmux handles it. TUI mode would need `vte` crate. | TUI only |
| Copy-paste | tmux copy mode. TUI mode would need clipboard crate. | TUI only |
| ANSI rendering | tmux renders natively. TUI mode needs VTE state machine. | TUI only |

---

## Files Removed

- `lib/pty-host.py` — replaced by `portable-pty`
- `lib/controller.js` — ported to Rust daemon
- `lib/output-extractors.js` — ported to Rust
- `lib/session-store.js` — ported to Rust
- `bin/cli.js` — replaced by Rust CLI

## Files Retained

- `orchestration-template.md` — injected into CLAUDE.md
- `.claude/settings.template.json` — injected into project
- `.github/workflows/publish-npm.yml` — updated for binary distribution
- `test/` — rewritten in Rust (`cargo test`)

---

## Effort Estimate

| Phase | Effort |
|-------|--------|
| Phase 0 | 1 day |
| Phase 1a | 2-3 days |
| Phase 1b | 2-3 days |
| Phase 1c | 1-2 days |
| Phase 2a | 2-3 days |
| Phase 2b | 2-3 days |
| Phase 2c | 2-3 days |
| Phase 3 | 3-5 days |
| **Total** | **15-23 days** |

Assumes Rust proficiency and familiarity with current cduo codebase.
