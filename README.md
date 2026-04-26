[English](README.md) | [한국어](README.ko.md)

# cduo

[![Build Status](https://github.com/hgwk/cduo/workflows/CI/badge.svg)](https://github.com/hgwk/cduo/actions)
[![npm version](https://img.shields.io/npm/v/@hgwk/cduo.svg)](https://www.npmjs.com/package/@hgwk/cduo)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Paired AI agent execution for Claude Code and OpenAI Codex in a split tmux workspace.

## What It Does

`cduo` runs two AI agent sessions side by side in a split `tmux` workspace, enabling paired agent execution with automatic message relaying between panes. It supports Claude Code (`claude`) and OpenAI Codex (`codex`) agents, detecting completions and forwarding context so both agents stay synchronized.

Built as a single Rust binary with an embedded daemon, PTY manager, message bus, and hook server. No Node.js or Python runtime required.

## Requirements

- `tmux`
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

Behavior summary:

- `cduo` is the same as `cduo start`
- `cduo start` defaults to Claude
- `cduo init` is only needed for Claude-oriented project context
- Codex sessions work without `cduo init`

## Daily Workflow

```bash
cduo doctor
cduo claude
```

Later, from the same project:

```bash
cduo resume
cduo status
cduo stop
```

If `cduo` starts from a non-interactive process, it creates the workspace and prints a `cduo resume ...` command instead of attaching immediately.

Operational notes:

- `cduo resume` requires an interactive terminal because it attaches to the running `tmux` workspace
- `cduo status --verbose` adds session id, agent, hook port, creation time, and pane attach-port details for diagnostics

Workspace selection rules:

- `cduo resume` and `cduo stop` first prefer a single workspace for the current project
- If multiple workspaces match the current project, `cduo` stops and asks you to choose one explicitly
- If no current-project workspace exists and only one workspace is active overall, `cduo` uses it automatically
- Explicit selectors can match a session name, session id, project name, or unique prefix shown by `cduo status`

## Commands

| Command | Purpose |
| --- | --- |
| `cduo` | Start a tmux split workspace with Claude defaults |
| `cduo help` or `cduo --help` | Show command help |
| `cduo start [claude\|codex] [yolo\|--yolo\|--full-access] [--new]` | Start or reconnect to a split workspace with the selected agent |
| `cduo claude [yolo\|--yolo\|--full-access] [--new]` | Start or reconnect to a Claude workspace |
| `cduo codex [yolo\|--yolo\|--full-access] [--new]` | Start or reconnect to a Codex workspace |
| `cduo doctor` | Check machine setup and current project readiness |
| `cduo resume [session]` | Reconnect to the current project workspace or the named one |
| `cduo status [--verbose]` | Show active cduo workspaces |
| `cduo stop [session]` | Stop the current project workspace or the named one |
| `cduo init` | Ensure the Claude `Stop` hook and create or prepend orchestration content in `CLAUDE.md` |
| `cduo init --force` | Overwrite `CLAUDE.md` and `.claude/settings.local.json` instead of merging |
| `cduo backup` | Back up orchestration-related files in the current project |
| `cduo update` | Update to the latest version |
| `cduo version` or `cduo --version` | Show the installed cduo version |
| `cduo uninstall` | Remove the injected Claude hook and orchestration content |

## Argument Rules

- Only one agent can be selected per session
- `yolo` and `--yolo` are equivalent
- `yolo` or `--yolo` cannot be combined with `--full-access`
- By default, `cduo claude` or `cduo codex` reconnects to the existing workspace for the same project, agent, and access mode
- Use `--new` only when you intentionally want a second workspace for the same project and agent
- After `start`, `claude` or `codex` can appear in any later position
- Unexpected extra start arguments are rejected instead of being ignored

Valid examples:

```bash
cduo
cduo update
cduo start
cduo start codex
cduo claude yolo
cduo codex --yolo
cduo codex --full-access
cduo codex --new
```

Rejected example:

```bash
cduo start claude codex
cduo codex nonsense
```

## Access Modes

- `cduo claude --full-access` launches Claude with `--permission-mode bypassPermissions`
- `cduo claude yolo` launches Claude with `--dangerously-skip-permissions`
- `cduo codex --full-access` launches Codex with the installed official OpenAI CLI's full-access equivalent
- `cduo codex yolo` launches Codex with the installed official OpenAI CLI's auto-approval equivalent

Codex option mapping depends on the installed official CLI:

- Newer builds use `--yolo` and `--sandbox danger-full-access`
- Older official builds use `--approval-mode full-auto` and `--dangerously-auto-approve-everything`

`cduo` detects both official variants before launch.

Reference for supported OpenAI Codex CLI options:

- [Codex CLI reference](https://developers.openai.com/codex/cli/reference)
- [Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)

## Agent Behavior

| Agent | Launch command | Completion detection | Files touched by `start` |
| --- | --- | --- | --- |
| Claude | `claude` | `Stop` hook + Claude transcript JSONL | may create or merge the Claude `Stop` hook |
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
- `cduo start` and `cduo claude ...` only manage the Claude `Stop` hook when Claude is selected
- `cduo codex ...` does not modify project files
- `cduo backup` writes timestamped copies into `.cduo/backups/`

## Relay Model

1. `cduo` starts an embedded daemon that manages the workspace.
2. The daemon launches the selected agent twice in direct PTYs with `TERMINAL_ID` and `ORCHESTRATION_PORT`.
3. `tmux` provides the split UI.
4. Claude sends completion events through the `Stop` hook to the embedded hook server.
5. Codex completions are read from Codex rollout JSONL files for the current workspace.
6. `MessageBus` deduplicates source/target/content deliveries and `PairRouter` forwards each agent response to its counterpart.
7. Relay output is written directly to the target PTY stdin; terminal UI output is not used as message content.

The daemon persists sessions using PID files and Unix sockets. Use `cduo resume` to reattach to the tmux session.

Preferred relay base port:

- `53333`

If the default local range is already busy, `cduo` automatically falls back to OS-assigned local ports.

Override the preferred base port if needed:

```bash
PORT=8080 cduo codex
```

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
- Run `cduo status` and confirm the workspace and controller are up
- If you need deeper diagnostics, run `cduo status --verbose`
- `cduo start`, `cduo resume`, and `cduo status` automatically remove stale workspace metadata before continuing
- For Claude, confirm the relay server logs show hook events
- For Codex, confirm a recent rollout JSONL exists under `~/.codex/sessions/` for the current project
- The target pane must accept stdin; `cduo` writes the relayed text and then sends Enter
- After upgrading `cduo`, restart the cduo session so the new daemon is actually running
- Confirm the `tmux` session is still running
- From the same project, `cduo resume` should reconnect to the expected workspace without an extra selector
- If attach fails after a workspace starts, the workspace usually keeps running; use the printed `cduo resume ...` command from an interactive terminal

Codex is installed but `cduo codex` is rejected:

- Check whether `codex --help` shows either the newer official options (`--yolo`, `--ask-for-approval`, `--sandbox`) or the older official options (`--approval-mode`, `full-auto`, `--dangerously-auto-approve-everything`)
- If not, install or upgrade the official CLI with `npm install -g @openai/codex@latest`
- Verify that your `PATH` resolves `codex` to the OpenAI binary

`tmux` layout mode fails immediately:

- macOS: `brew install tmux`
- Ubuntu or Debian: `sudo apt install tmux`
- Fedora: `sudo dnf install tmux`
- Arch: `sudo pacman -S tmux`

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

Run the local end-to-end smoke test:

```bash
scripts/e2e-test.sh
```

The release binary will be at `target/release/cduo`.

Project layout:

```text
cduo/
├── src/
│   ├── main.rs           # CLI entry point
│   ├── cli.rs            # Command definitions and parsing
│   ├── daemon.rs         # Embedded daemon and session management
│   ├── hook.rs           # HTTP hook server for Claude
│   ├── pty.rs            # PTY management (portable-pty)
│   ├── message.rs        # Relay message model
│   ├── message_bus.rs    # Deduping message bus
│   ├── pair_router.rs    # 1:1 routing policy
│   ├── session.rs        # Session metadata and persistence
│   ├── tmux.rs           # tmux session helpers
│   └── transcripts/      # Agent transcript readers
├── templates/
│   ├── claude-settings.json
│   └── orchestration.md
├── npm/
│   ├── install.js
│   └── package.json
├── scripts/
│   └── e2e-test.sh
├── docs/
│   └── architecture.md
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
- The npm package is a thin wrapper that downloads the appropriate binary on install
- Release tags must match the versions in `Cargo.toml` and `npm/package.json`

## License

MIT
