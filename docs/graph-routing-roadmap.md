# Graph Routing Roadmap

This roadmap extends the current native 1:1 relay into configurable `1:N` and
`N:N` agent communication without reintroducing tmux, terminal scraping, or
hard-coded loop gates.

## Current Baseline

- Two foreground native panes rendered by ratatui.
- Transcript-sourced relay only:
  - Claude: Stop hook + transcript JSONL.
  - Codex: rollout JSONL polling + pane binding.
- `MessageBus` handles pub/sub and duplicate suppression.
- `PairRouter` maps `a -> b` and `b -> a`.
- Relay delivery uses bracketed paste into the target PTY.

The next routing layer should preserve these properties. The UI owns PTYs; the
relay owns transcript events and routing decisions.

## Target Capabilities

### 1:N Fan-Out

One source node can send the same assistant message to multiple target nodes.

Examples:

```text
human -> architect
architect -> executor-1
architect -> executor-2
architect -> reviewer
```

Required behavior:

- A single source transcript event becomes one logical `Message`.
- The router expands that message into multiple target deliveries.
- Dedup keys remain per `(source, target, content, origin)` so one slow or
duplicate target does not suppress delivery to the others.
- Delivery failures are logged per target and do not cancel the whole fan-out.

### N:N Mesh

Any node can publish to any configured subset of nodes.

Examples:

```text
planner -> architect, executor
executor -> reviewer
reviewer -> executor, planner
```

Required behavior:

- Routes are explicit; no implicit broadcast by default.
- Self-routes are rejected unless explicitly enabled for a future special mode.
- The router returns an ordered target list for deterministic logs and tests.
- The message model keeps the original source node, even after fan-out.

## Routing Model

Replace `PairRouter` with a graph router while keeping the same core relay
pipeline.

```rust
pub struct RouteGraph {
    routes: HashMap<NodeId, Vec<NodeId>>,
}

impl RouteGraph {
    pub fn targets_for(&self, source: &str) -> &[NodeId];
}
```

Initial config shape:

```toml
[nodes.architect]
agent = "claude"

[nodes.executor]
agent = "codex"

[nodes.reviewer]
agent = "claude"

[routes]
architect = ["executor", "reviewer"]
executor = ["reviewer"]
reviewer = ["architect"]
```

CLI shortcuts can map onto this model:

```bash
cduo start claude codex                 # current 1:1 shorthand
cduo start --graph cduo.toml            # explicit graph mode
cduo start --nodes claude,codex,claude  # generated node IDs: a,b,c
```

## Implementation Phases

### Phase 1: Internal Router Generalization

- Introduce `route_graph.rs`.
- Keep the existing 2-pane UI.
- Adapt the relay to call `targets_for(source)` instead of `counterpart`.
- Keep `PairRouter` only as a thin compatibility constructor or remove it after
  tests migrate.
- Add tests for:
  - `a -> [b, c]` fan-out.
  - `a -> []` no-op.
  - unknown source no-op with log.
  - self-route rejection.
  - deterministic target order.

Acceptance:

- Existing 1:1 behavior is unchanged.
- The relay core can produce more than one delivery for a single source message
  in unit tests.

### Phase 2: Native Multi-Pane Layout

- Replace fixed pane IDs `a`/`b` with `Vec<NodePane>`.
- Add focus navigation across N panes.
- Add a layout strategy:
  - 2 nodes: horizontal split.
  - 3 nodes: primary left, two stacked right.
  - 4+ nodes: grid.
- Keep PTY ownership in the UI thread.
- Keep transcript binding keyed by node ID, not pane position.

Acceptance:

- `cduo start --nodes claude,codex,claude` starts three live panes.
- User input goes only to the focused node.
- Relay writes can target any node by ID.

### Phase 3: Configurable Delivery Rules

Add explicit rule options without hard-coded loop prevention:

- `mode = "fanout" | "mesh" | "manual"`
- `allow_self = false`
- `include_human_messages = false`
- `dedup_window_ms = 10000`
- `submit_delay_ms` per agent or per node.

These are transport and routing policies, not conversation-control gates. The
system should not decide that a conversation has gone on "too long"; users and
config own that behavior.

Acceptance:

- Rules are loaded from config.
- Defaults reproduce current 1:1 behavior.
- Tests prove rule parsing and route expansion.

### Phase 4: Operator Controls

Add runtime controls that help a human steer the graph:

- Toggle route enabled/disabled.
- Inject a message into one selected node.
- Inject a message into multiple selected nodes.
- View a compact route graph overlay.
- Save session logs with node IDs and delivery IDs.

Acceptance:

- A user can pause one edge without stopping the whole session.
- Logs show `message_id`, `source`, `target`, `content_hash`, and delivery
  result for each edge.

## Non-Goals

- No tmux fallback.
- No PTY screen scraping as a message source.
- No regex cleanup of terminal UI as relay content.
- No hard-coded loop limits or automatic conversation stopping.
- No background daemon unless a separate product requirement appears.

## Open Decisions

- Config file location and naming: project-local `cduo.toml` vs explicit
  `--graph` only.
- Whether `human` should become a first-class source node in graph mode.
- Whether node roles should be free-form labels or a constrained enum.
- Whether delivery should remain sequential per source event or become
  concurrent per target.
