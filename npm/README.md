# cduo

Paired AI agent execution for Claude Code and OpenAI Codex in a native split-pane terminal UI.

This npm package installs the platform-specific `cduo` Rust binary from GitHub Releases.
The package itself is a small installer wrapper; the runtime binary is built and
uploaded by the GitHub Release workflow.

## Install

```bash
npm install -g @hgwk/cduo
```

## Usage

```bash
cduo doctor
cduo start claude codex
```

Native UI controls: `Ctrl-W` switches panes, `Ctrl-P` pauses/resumes relay
delivery, `Ctrl-Q` quits, `PageUp/PageDown` scroll the focused pane, and mouse
drag copies text from one pane via OSC52.

Full documentation is available in the project repository:

https://github.com/hgwk/cduo

## Release Notes

`@hgwk/cduo` is published from GitHub Actions using npm Trusted Publishing
(OIDC). No long-lived npm token is required for release automation.
