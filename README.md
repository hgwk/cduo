[English](README.md) | [한국어](README.ko.md)

# cduo

[![Build Status](https://github.com/hgwk/cduo/workflows/CI/badge.svg)](https://github.com/hgwk/cduo/actions)
[![npm version](https://img.shields.io/npm/v/@hgwk/cduo.svg)](https://www.npmjs.com/package/@hgwk/cduo)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Paired AI agent execution for Claude Code and OpenAI Codex in a native split terminal UI.

## What It Does

`cduo` runs two AI agent sessions side by side in a native split-pane terminal UI, enabling paired agent execution with automatic message relaying between panes. It supports Claude Code (`claude`) and OpenAI Codex (`codex`) agents, detecting completions and forwarding context so both agents stay synchronized.

Built as a single Rust binary with a native TUI, PTY manager, message bus, and hook server. No Node.js or Python runtime is required by `cduo` itself.

## Requirements

- Claude Code CLI on your `PATH` as `claude` for Claude sessions
- OpenAI Codex CLI on your `PATH` as `codex` for Codex sessions

Install Codex if needed:

```bash
npm install -g @openai/codex@latest
```

## Supported Platforms

- macOS (x86_64, arm64)
- Linux (x86_64, aarch64)

Windows support is planned for v2.1.

## Quick Start

Install globally:

```bash
npm install -g @hgwk/cduo
```

This downloads the appropriate Rust binary for your platform from GitHub Releases.

Check your environment once:

```bash
cduo doctor
```

Claude:

```bash
cd /path/to/project
cduo init
cduo claude
```

Codex:

```bash
cd /path/to/project
cduo codex
```

Mixed Claude + Codex:

```bash
cd /path/to/project
cduo init   # needed for Claude relay
cduo start claude codex
```

Behavior summary:

- `cduo` is the same as `cduo start`
- `cduo start` defaults to Claude
- `cduo init` is only needed for Claude-oriented project context
- Codex sessions work without `cduo init`
- Native sessions are foreground processes; closing the UI stops the agents

## Daily Workflow

```bash
cduo doctor
cduo start claude codex
```

Native UI controls:

- `Ctrl-W`: switch focus between panes
- `Ctrl-Shift-W`: switch focus in the opposite direction
- `Ctrl-P`: pause or resume automatic relay delivery
- `Ctrl-L`: toggle split layout between columns and rows
- `Ctrl-Q`: quit the native UI and stop both agents
- `PageUp` / `PageDown`: scroll the focused pane
- Mouse wheel: scroll the pane under the cursor
- Mouse drag: select text inside one pane; release to copy the selected text via OSC52

## Commands

| Command | Purpose |
| --- | --- |
| `cduo` | Start the native split UI with Claude defaults |
| `cduo help` or `cduo --help` | Show command help |
| `cduo start [claude\|codex] [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | Start the native split UI; optional second agent selects pane B |
| `cduo claude [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | Start a native pair with Claude in pane A |
| `cduo codex [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | Start a native pair with Codex in pane A |
| `cduo doctor` | Check machine setup and current project readiness |
| `cduo status [--verbose]` | Report native foreground-session behavior |
| `cduo init` | Ensure the Claude `Stop` hook and create or prepend orchestration content in `CLAUDE.md` |
| `cduo init --force` | Overwrite `CLAUDE.md` and `.claude/settings.local.json` instead of merging |
| `cduo backup` | Back up orchestration-related files in the current project |
| `cduo update` | Update to the latest version |
| `cduo version` or `cduo --version` | Show the installed cduo version |
| `cduo uninstall` | Remove the injected Claude hook and orchestration content |

## Argument Rules

- `cduo start` accepts one optional agent for pane A and one optional peer agent for pane B
- `--yolo` cannot be combined with `--full-access`
- Native mode starts a fresh foreground session each time; there is no background workspace to attach or resume
- `--new` / `--new-session` is accepted for CLI compatibility, but is currently a no-op in native mode
- Unexpected extra start arguments are rejected instead of being ignored

Valid examples:

```bash
cduo
cduo update
cduo start
cduo start codex
cduo start claude codex
cduo claude codex
cduo codex claude
cduo codex claude --split rows
cduo start --new claude codex
cduo claude --yolo
cduo codex --yolo
cduo codex --full-access
cduo codex --new
```

Rejected example:

```bash
cduo start claude codex claude
cduo codex nonsense
```

## Access Modes

- `cduo claude --full-access` launches Claude with `--permission-mode bypassPermissions`
- `cduo claude --yolo` launches Claude with `--dangerously-skip-permissions`
- `cduo codex --full-access` launches Codex with the installed official OpenAI CLI's full-access equivalent
- `cduo codex --yolo` launches Codex with the installed official OpenAI CLI's auto-approval equivalent

Codex option mapping targets the installed official CLI:

- `--full-access` launches Codex with `--sandbox danger-full-access --ask-for-approval never`
- `--yolo` launches Codex with `--dangerously-bypass-approvals-and-sandbox`

Reference for supported OpenAI Codex CLI options:

- [Codex CLI reference](https://developers.openai.com/codex/cli/reference)
- [Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)

## Agent Behavior

| Agent | Launch command | Completion detection | Files touched by `start` |
| --- | --- | --- | --- |
| Claude | `claude` | `Stop` hook + Claude transcript JSONL | none; run `cduo init` to install the hook |
| Codex | `codex` | Codex rollout JSONL | none |

When Codex is selected, `cduo` checks that `codex` resolves to the official OpenAI CLI before launching.

## What Commands Modify

`cduo init` may create or update:

```text
your-project/
├── .cduo/
│   └── backups/
├── .claude/
│   └── settings.local.json
├── CLAUDE.md
└── ...
```

Command behavior:

- `cduo init` manages both `.claude/settings.local.json` and `CLAUDE.md`
- `cduo start`, `cduo claude ...`, and `cduo codex ...` do not modify project files
- `cduo backup` writes timestamped copies into `.cduo/backups/`

## Relay Model

1. `cduo` starts a native split-pane TUI.
2. The native runtime launches the selected agents in direct PTYs with `TERMINAL_ID` and `ORCHESTRATION_PORT`.
3. `ratatui` + `vt100` render the two PTYs directly; there is no tmux fallback.
4. Claude sends completion events through the `Stop` hook to the embedded hook server.
5. Codex completions are read from Codex rollout JSONL files for the current workspace.
6. `MessageBus` deduplicates source/target/content deliveries and `PairRouter` forwards each agent response to its counterpart.
7. Relay output is written directly to the target PTY stdin; terminal UI output is not used as message content.

Preferred relay base port:

- `53333`

If the default local range is already busy, `cduo` automatically falls back to OS-assigned local ports.

Override the preferred base port if needed:

```bash
CDUO_PORT=8080 cduo codex
```

`PORT` is also accepted for hosting environments that already provide it, but `CDUO_PORT` takes precedence.

## Backup, Uninstall, and Update

Create a backup manually:

```bash
cduo backup
```

Remove orchestration changes from the current project:

```bash
cduo uninstall
```

`cduo uninstall` backs up current orchestration files first, then:

- Removes the Claude `Stop` hook from `.claude/settings.local.json`
- Removes the bundled Claude permission default if it matches the cduo template
- Removes the prepended orchestration block from `CLAUDE.md`
- Deletes `CLAUDE.md` entirely if it only contains the bundled orchestration template

Update the installed CLI:

```bash
cduo update
```

`cduo update` downloads the latest binary from GitHub Releases.

## Troubleshooting

Messages are not relaying:

- Run `cduo doctor` first and confirm the runtime is healthy
- Confirm both panes are visible in the native UI
- If a Claude pane is involved, run `cduo init` once in that project so the `Stop` hook exists
- For Claude, confirm the relay server logs show hook events
- For Codex, confirm a recent rollout JSONL exists under `~/.codex/sessions/` for the current project
- The target pane must accept stdin; `cduo` writes the relayed text and then sends Enter
- After upgrading `cduo`, restart the native UI so the new binary is actually running

Codex is installed but `cduo codex` is rejected:

- Check whether `codex --help` shows either the newer official options (`--yolo`, `--ask-for-approval`, `--sandbox`) or the older official options (`--approval-mode`, `full-auto`, `--dangerously-auto-approve-everything`)
- If not, install or upgrade the official CLI with `npm install -g @openai/codex@latest`
- Verify that your `PATH` resolves `codex` to the OpenAI binary

The terminal starts in the wrong directory:

- Run `cduo claude` or `cduo codex` from the intended project root

Claude is missing the orchestration context:

- Run `cduo init`
- Note that only Claude flows manage `CLAUDE.md`

Claude shows `SessionStart:startup hook error` before the prompt:

- Run `cduo doctor` and check the `Claude startup hooks` line
- This warning usually comes from a third-party Claude plugin such as `claude-mem`, not from the cduo `Stop` hook
- Update or patch the offending plugin so its `SessionStart` hook returns JSON only

## Development

Build from source:

```bash
git clone https://github.com/hgwk/cduo.git
cd cduo
cargo build --release
```

Run tests:

```bash
cargo test
```

Current automated coverage is `cargo test` (unit + in-process relay integration tests under `src/native/relay.rs`). A full TUI end-to-end harness for the foreground native runtime is not currently wired.

The release binary will be at `target/release/cduo`.

Project layout:

```text
cduo/
├── src/
│   ├── main.rs           # CLI entry point
│   ├── cli.rs            # Command definitions and parsing
│   ├── native/           # Native split-pane TUI runtime (PTY + ratatui + relay loop)
│   │   ├── runtime.rs    # Two-pane main loop; spawns hook server and relay task
│   │   ├── pane.rs       # Per-pane PTY + vt100 parser
│   │   ├── ui.rs         # vt100 → ratatui rendering
│   │   ├── input.rs      # Key encoding and global action classification
│   │   └── relay.rs      # In-process relay loop driving the message bus
│   ├── relay_core.rs     # Pure helpers: transcript reads, codex rollout discovery, dedup
│   ├── hook.rs           # HTTP hook server for Claude Stop events
│   ├── message.rs        # Relay message model
│   ├── message_bus.rs    # Deduping message bus
│   ├── pair_router.rs    # 1:1 routing policy
│   ├── session.rs        # State directory resolution
│   ├── project.rs        # `init` / `doctor` / `backup` / `uninstall` for project files
│   └── transcripts/      # Agent transcript readers (claude, codex)
├── templates/
│   ├── claude-settings.json
│   └── orchestration.md
├── npm/
│   ├── install.js
│   └── package.json
├── docs/
│   ├── architecture.md
│   └── graph-routing-roadmap.md
├── Cargo.toml
├── Cargo.lock
├── .github/
│   └── workflows/
│       ├── rust-ci.yml
│       └── release.yml
├── LICENSE
├── README.md
└── README.ko.md
```

## Release Flow

- GitHub repository: `hgwk/cduo`
- npm package: `@hgwk/cduo`
- GitHub Releases hosts platform-specific Rust binaries
- `.github/workflows/rust-ci.yml` runs tests on every push and pull request
- `.github/workflows/release.yml` builds and publishes binaries on version tags
- The npm package is published from GitHub Actions through npm Trusted Publishing (OIDC), not a long-lived `NPM_TOKEN`
- The npm package is a thin wrapper that downloads the appropriate binary from GitHub Releases on install
- Release tags must match the versions in `Cargo.toml` and `npm/package.json`
- npm Trusted Publisher configuration:
  - Publisher: `GitHub Actions`
  - Organization or user: `hgwk`
  - Repository: `cduo`
  - Workflow filename: `release.yml`
  - Environment name: empty

## Roadmap

- Current stable mode: transcript-sourced native `1:1` relay
- Planned extension: configurable `1:N` fan-out and `N:N` graph routing
- See [`docs/graph-routing-roadmap.md`](docs/graph-routing-roadmap.md)

## License

MIT
