# Footer Visual Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply 15 visual/UX refinements (items 1–15 from prior ideation) to cduo's TUI footer/header/divider — making relay state "feel alive" while keeping the dense single-row status bar readable.

**Architecture:** All formatting logic moves into pure helpers in a new `src/native/footer.rs` module so behaviors are unit-testable. `render.rs` calls these helpers and adds new token style branches. `runtime.rs` gains lightweight counters (traffic, hook pings, error timestamps) and forces redraws on a slow tick for time-driven glyphs. No new dependencies.

**Tech Stack:** Rust, ratatui, tokio mpsc, std::time::Instant. Existing test pattern: `#[cfg(test)] mod tests` blocks at the bottom of each `.rs` file.

---

## File Structure

- **Create:** `src/native/footer.rs` — pure formatters & tests
  - `mode_glyph(mode)`, `focus_caret_for(pane, focus)`, `build_channel_dot()`
  - `uptime_label(elapsed)`, `pingpong_dot(elapsed)`, `broadcast_caret_glyph(elapsed)`
  - `traffic_sparkline(samples)`, `direction_arrow(dir, recent_hit)`, `activity_dot(rate)`
  - `error_toast_fade(msg, elapsed)`, `hook_ping_glyph(since_last)`
  - `queue_gauge_glyph` (move from runtime.rs)
- **Modify:** `src/native/mod.rs` — `pub(crate) mod footer;`
- **Modify:** `src/native/runtime.rs` — counters, forced-tick on time-driven states, call new helpers
- **Modify:** `src/native/render.rs` — new token style branches, header/divider tweaks, log-ticker support
- **Modify:** `src/native/input.rs` (or wherever Ctrl-? maps if added) — log ticker key binding

---

## Task 1: Pure formatter scaffold + static glyphs (items 4, 6, 13, 14)

**Files:**
- Create: `src/native/footer.rs`
- Modify: `src/native/mod.rs`
- Modify: `src/native/runtime.rs` (move `queue_gauge_glyph`, wire `mode_glyph`, separator unification, build dot)
- Modify: `src/native/render.rs` (focus caret in header)

- [ ] **Step 1: Write failing tests for static helpers**

Create `src/native/footer.rs`:

```rust
use std::time::Duration;

pub(crate) fn mode_glyph(mode: &str) -> &'static str {
    match mode {
        "ON" => "▶",
        "PAUSE" => "⏸",
        "STOP" => "⏹",
        "BROADCAST" => "📡",
        _ => "·",
    }
}

pub(crate) fn focus_caret(is_focused: bool) -> &'static str {
    if is_focused { "◀" } else { " " }
}

pub(crate) fn build_channel_dot() -> &'static str {
    let v = env!("CARGO_PKG_VERSION");
    if v.contains("-dev") || v.contains("-rc") {
        "●dev"
    } else if v.contains("-beta") {
        "●beta"
    } else {
        "●"
    }
}

pub(crate) fn queue_gauge_glyph(n: usize) -> &'static str {
    match n {
        0 => "",
        1 => " ▁",
        2 => " ▂",
        3..=4 => " ▃",
        5..=8 => " ▄",
        9..=16 => " ▅",
        17..=32 => " ▆",
        33..=64 => " ▇",
        _ => " █",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_glyph_known_modes() {
        assert_eq!(mode_glyph("ON"), "▶");
        assert_eq!(mode_glyph("PAUSE"), "⏸");
        assert_eq!(mode_glyph("STOP"), "⏹");
        assert_eq!(mode_glyph("BROADCAST"), "📡");
        assert_eq!(mode_glyph("???"), "·");
    }

    #[test]
    fn focus_caret_toggles() {
        assert_eq!(focus_caret(true), "◀");
        assert_eq!(focus_caret(false), " ");
    }

    #[test]
    fn queue_gauge_scale() {
        assert_eq!(queue_gauge_glyph(0), "");
        assert_eq!(queue_gauge_glyph(1), " ▁");
        assert_eq!(queue_gauge_glyph(8), " ▄");
        assert_eq!(queue_gauge_glyph(65), " █");
        assert_eq!(queue_gauge_glyph(10_000), " █");
    }

    #[test]
    fn build_channel_dot_non_empty() {
        // Cannot assert exact value without knowing version, but must be non-empty
        assert!(!build_channel_dot().is_empty());
    }
}
```

Modify `src/native/mod.rs` to add (find existing module list and append):
```rust
pub(crate) mod footer;
```

- [ ] **Step 2: Run the new tests — confirm they pass**

Run: `cargo test -p cduo native::footer`
Expected: 4 passed.

- [ ] **Step 3: Remove the now-duplicated `queue_gauge_glyph` from `runtime.rs`**

In `src/native/runtime.rs`, delete the local `fn queue_gauge_glyph(...)` and add `use crate::native::footer::queue_gauge_glyph;` at the top.

- [ ] **Step 4: Unify separator + add mode glyph + build dot in `footer_with_relay_status`**

Replace the body of `footer_with_relay_status` in `src/native/runtime.rs` with:

```rust
fn footer_with_relay_status(
    message: &str,
    relay_paused: bool,
    queued_writes: usize,
    a_to_b_enabled: bool,
    b_to_a_enabled: bool,
    relay_auto_stopped: bool,
    heartbeat: bool,
) -> String {
    use crate::native::footer::{mode_glyph, queue_gauge_glyph};
    let mode = if relay_auto_stopped {
        "STOP"
    } else if relay_paused {
        "PAUSE"
    } else {
        "ON"
    };
    let glyph = mode_glyph(mode);
    let pulse = if relay_paused {
        if heartbeat { " ●" } else { " ○" }
    } else {
        ""
    };
    let gauge = queue_gauge_glyph(queued_writes);
    let a_to_b = if a_to_b_enabled { "ON" } else { "OFF" };
    let b_to_a = if b_to_a_enabled { "ON" } else { "OFF" };
    format!(
        " {glyph} relay[{mode}]{pulse} · q[{queued_writes}]{gauge} · A=>B[{a_to_b}] · B=>A[{b_to_a}] · {}",
        message.trim()
    )
}
```

Note the unified `·` separators replacing the prior mix of spaces and `|`.

- [ ] **Step 5: Add focus caret + build dot to header in `render.rs`**

In `src/native/render.rs`, replace the header Paragraph construction with:

```rust
use crate::native::footer::{build_channel_dot, focus_caret};

let header_text = format!(
    " cduo · A{}:{} | B{}:{} · {} ",
    focus_caret(focus.0 == PaneId::A),
    panes[0].agent,
    focus_caret(focus.0 == PaneId::B),
    panes[1].agent,
    build_channel_dot(),
);
frame.render_widget(
    Paragraph::new(header_text)
        .style(Style::default().add_modifier(Modifier::BOLD)),
    header_area,
);
```

Add `◀` (focus caret) and `●` (channel dot) to the token style branches in `footer_token_style`:

```rust
if matches!(token, "◀" | "▶" | "⏸" | "⏹") {
    return Some(Style::default().fg(Color::Yellow));
}
if token.starts_with('●') {
    return Some(Style::default().fg(Color::Green));
}
```

- [ ] **Step 6: Build + verify visually**

Run: `cargo build --release`
Expected: success, no warnings.
Manual: `./target/release/cduo` — header shows `◀` next to focused pane, footer leads with `▶ relay[ON]`, version row ends `… v2.0.8`.

- [ ] **Step 7: Commit**

```bash
git add src/native/footer.rs src/native/mod.rs src/native/runtime.rs src/native/render.rs
git commit -m "Add footer pure-formatter module with mode glyph, focus caret, build dot"
```

---

## Task 2: Time-driven heartbeats — uptime, STOP blink, ping-pong, broadcast caret (items 3, 5, 7, 10)

**Files:**
- Modify: `src/native/footer.rs` (add helpers + tests)
- Modify: `src/native/runtime.rs` (pass `runtime_start` everywhere needed, change forced-tick to cover STOP/BROADCAST)

- [ ] **Step 1: Add helpers with tests**

Append to `src/native/footer.rs`:

```rust
pub(crate) fn uptime_label(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub(crate) fn pingpong_dot(elapsed: Duration) -> &'static str {
    // 4-frame ping-pong over a 2s cycle, both directions active only
    match (elapsed.as_millis() / 500) % 4 {
        0 => "·>",
        1 => "·>·",
        2 => "<·",
        _ => "·<·",
    }
}

pub(crate) fn broadcast_caret_glyph(elapsed: Duration) -> &'static str {
    match (elapsed.as_millis() / 350) % 3 {
        0 => "▏",
        1 => "▎",
        _ => "▍",
    }
}

pub(crate) fn stop_warn_glyph(elapsed: Duration) -> &'static str {
    // 4Hz blink (faster than pause heartbeat)
    if (elapsed.as_millis() / 250) % 2 == 0 { "!" } else { " " }
}
```

Append tests:

```rust
#[test]
fn uptime_label_formats() {
    assert_eq!(uptime_label(Duration::from_secs(0)), "00:00");
    assert_eq!(uptime_label(Duration::from_secs(75)), "01:15");
    assert_eq!(uptime_label(Duration::from_secs(3725)), "1h02m");
}

#[test]
fn pingpong_cycles_through_four_frames() {
    let frames: Vec<&str> = (0..4)
        .map(|i| pingpong_dot(Duration::from_millis(i * 500)))
        .collect();
    assert_eq!(frames, vec!["·>", "·>·", "<·", "·<·"]);
}

#[test]
fn stop_warn_blinks() {
    assert_eq!(stop_warn_glyph(Duration::from_millis(0)), "!");
    assert_eq!(stop_warn_glyph(Duration::from_millis(250)), " ");
}

#[test]
fn broadcast_caret_widens_or_narrows() {
    let a = broadcast_caret_glyph(Duration::from_millis(0));
    let b = broadcast_caret_glyph(Duration::from_millis(350));
    let c = broadcast_caret_glyph(Duration::from_millis(700));
    assert!(a != b && b != c);
}
```

- [ ] **Step 2: Run tests — expect pass**

Run: `cargo test -p cduo native::footer::tests`
Expected: 8 passed (4 prior + 4 new).

- [ ] **Step 3: Wire into runtime — extend forced-tick condition**

In `src/native/runtime.rs`, change the slow-tick gate to fire whenever any time-driven glyph is live:

```rust
let needs_tick = relay_paused || relay_auto_stopped || broadcast_input.is_some();
if needs_tick && !dirty && last_frame.elapsed() >= Duration::from_millis(250) {
    dirty = true;
}
```

- [ ] **Step 4: Use `stop_warn_glyph` and uptime label in `footer_with_relay_status`**

Extend signature to accept `elapsed: Duration`, then in the body:

```rust
let warn = if relay_auto_stopped {
    crate::native::footer::stop_warn_glyph(elapsed)
} else { "" };
let uptime = crate::native::footer::uptime_label(elapsed);
// in format!: " {glyph}{warn} relay[{mode}]{pulse} · q[...]... · up {uptime} · {msg}"
```

Update the single call site in the main loop to pass `runtime_start.elapsed()`.

- [ ] **Step 5: Use `broadcast_caret_glyph` in `broadcast_input_footer`**

Change `broadcast_input_footer` to accept `elapsed: Duration`:

```rust
fn broadcast_input_footer(buffer: &str, elapsed: Duration) -> String {
    let caret = crate::native::footer::broadcast_caret_glyph(elapsed);
    format!(" broadcast> {buffer}{caret} · Enter: send · Esc: cancel ")
}
```

Update all 2 call sites (search `broadcast_input_footer(`) to pass `runtime_start.elapsed()`.

- [ ] **Step 6: Use `pingpong_dot` when both routes ON and relay ON**

In `footer_with_relay_status`, when `!relay_paused && !relay_auto_stopped && a_to_b_enabled && b_to_a_enabled`, replace the static `A=>B[ON] · B=>A[ON]` rendering with `A{pp}B` where `pp = pingpong_dot(elapsed)`. Keep the bracketed form for the off cases (so the styler still highlights state changes).

Also extend `needs_tick`:

```rust
let needs_tick = relay_paused || relay_auto_stopped || broadcast_input.is_some()
    || (a_to_b_enabled && b_to_a_enabled);
```

- [ ] **Step 7: Style `!` warn glyph red, ping-pong dots dark gray (no special color = default)**

In `render.rs` `footer_token_style`, add:

```rust
if token == "!" {
    return Some(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
}
```

- [ ] **Step 8: Build + manual verification**

Run: `cargo build`. Manual: pause relay → see ●/○; trigger auto-stop (kill a pane child) → see `⏹! relay[STOP]` with `!` blinking red; start broadcast (Ctrl-Y) → caret pulses; idle relay → ping-pong moves.

- [ ] **Step 9: Commit**

```bash
git add src/native/footer.rs src/native/runtime.rs src/native/render.rs
git commit -m "Add time-driven footer glyphs: uptime, STOP warn, ping-pong, broadcast caret"
```

---

## Task 3: Traffic counters — direction arrow pulse, sparkline, per-pane activity (items 1, 2, 11)

**Files:**
- Modify: `src/native/footer.rs` (sparkline + direction_arrow + activity_dot helpers + tests)
- Modify: `src/native/runtime.rs` (per-direction byte counters + 1s rotating samples)

- [ ] **Step 1: Add helpers + tests**

Append to `footer.rs`:

```rust
pub(crate) fn traffic_sparkline(samples: &[u64]) -> String {
    let max = samples.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return "▁".repeat(samples.len().min(8));
    }
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    samples
        .iter()
        .take(8)
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * 7.0).round() as usize;
            bars[idx.min(7)]
        })
        .collect()
}

pub(crate) fn direction_arrow(active: bool, recently_hit: bool) -> &'static str {
    match (active, recently_hit) {
        (false, _) => "─x─",
        (true, true) => "━▶━",
        (true, false) => "─▶─",
    }
}

pub(crate) fn activity_dot(bytes_last_sec: u64) -> &'static str {
    match bytes_last_sec {
        0 => "·",
        1..=128 => "∘",
        _ => "●",
    }
}
```

Append tests:

```rust
#[test]
fn sparkline_empty_is_baseline() {
    assert_eq!(traffic_sparkline(&[0, 0, 0]), "▁▁▁");
}

#[test]
fn sparkline_scales_to_max() {
    let s = traffic_sparkline(&[0, 50, 100]);
    assert_eq!(s.chars().count(), 3);
    let chars: Vec<char> = s.chars().collect();
    assert_eq!(chars[0], '▁');
    assert_eq!(chars[2], '█');
}

#[test]
fn direction_arrow_states() {
    assert_eq!(direction_arrow(false, false), "─x─");
    assert_eq!(direction_arrow(true, false), "─▶─");
    assert_eq!(direction_arrow(true, true), "━▶━");
}

#[test]
fn activity_dot_thresholds() {
    assert_eq!(activity_dot(0), "·");
    assert_eq!(activity_dot(50), "∘");
    assert_eq!(activity_dot(10_000), "●");
}
```

- [ ] **Step 2: Run new tests**

Run: `cargo test -p cduo native::footer`
Expected: 12 passed.

- [ ] **Step 3: Add traffic counter state to main loop**

In `src/native/runtime.rs` before `'main: loop`:

```rust
struct TrafficCounters {
    a_to_b_bytes: u64,         // bytes targeting pane B == A→B
    b_to_a_bytes: u64,
    last_a_to_b_at: Option<Instant>,
    last_b_to_a_at: Option<Instant>,
    samples_a_to_b: std::collections::VecDeque<u64>, // last 8 seconds
    samples_b_to_a: std::collections::VecDeque<u64>,
    last_sample_at: Instant,
}

let mut traffic = TrafficCounters {
    a_to_b_bytes: 0,
    b_to_a_bytes: 0,
    last_a_to_b_at: None,
    last_b_to_a_at: None,
    samples_a_to_b: std::collections::VecDeque::from(vec![0u64; 8]),
    samples_b_to_a: std::collections::VecDeque::from(vec![0u64; 8]),
    last_sample_at: Instant::now(),
};
```

- [ ] **Step 4: Update `drain_relay_writes` to record traffic**

Change its signature to take `&mut TrafficCounters`. On every successful write:

```rust
match target.as_str() {
    "b" => {
        traffic.a_to_b_bytes += bytes.len() as u64;
        traffic.last_a_to_b_at = Some(Instant::now());
    }
    "a" => {
        traffic.b_to_a_bytes += bytes.len() as u64;
        traffic.last_b_to_a_at = Some(Instant::now());
    }
    _ => {}
}
```

- [ ] **Step 5: Rotate samples once per second in main loop**

After the dirty/draw block:

```rust
if traffic.last_sample_at.elapsed() >= Duration::from_secs(1) {
    traffic.samples_a_to_b.pop_front();
    traffic.samples_a_to_b.push_back(traffic.a_to_b_bytes);
    traffic.samples_b_to_a.pop_front();
    traffic.samples_b_to_a.push_back(traffic.b_to_a_bytes);
    traffic.a_to_b_bytes = 0;
    traffic.b_to_a_bytes = 0;
    traffic.last_sample_at = Instant::now();
    dirty = true;
}
```

- [ ] **Step 6: Render sparkline + arrows + activity dots**

Extend `footer_with_relay_status` signature to take `&TrafficCounters` and a `now: Instant`. Replace the route block:

```rust
let pulse_a_to_b = traffic.last_a_to_b_at
    .map(|t| now.duration_since(t) < Duration::from_millis(200))
    .unwrap_or(false);
let pulse_b_to_a = traffic.last_b_to_a_at
    .map(|t| now.duration_since(t) < Duration::from_millis(200))
    .unwrap_or(false);
let arrow_ab = direction_arrow(a_to_b_enabled, pulse_a_to_b);
let arrow_ba = direction_arrow(b_to_a_enabled, pulse_b_to_a);
let spark_ab: Vec<u64> = traffic.samples_a_to_b.iter().copied().collect();
let spark_ba: Vec<u64> = traffic.samples_b_to_a.iter().copied().collect();
let act_a = activity_dot(*traffic.samples_b_to_a.back().unwrap_or(&0)); // A receives B→A
let act_b = activity_dot(*traffic.samples_a_to_b.back().unwrap_or(&0));
// in format!:
//   "A{act_a} {arrow_ba} {spark_ba} | {spark_ab} {arrow_ab} B{act_b}"
```

Add `needs_tick` extension: traffic causes redraw automatically via sample rotation.

- [ ] **Step 7: Style new tokens in `render.rs`**

In `footer_token_style`:

```rust
if matches!(token, "─▶─" | "━▶━" | "─x─") {
    let bold = token == "━▶━";
    let color = if token == "─x─" { Color::Red } else { Color::Green };
    let mut s = Style::default().fg(color);
    if bold { s = s.add_modifier(Modifier::BOLD); }
    return Some(s);
}
if matches!(token, "·" | "∘" | "●") {
    return Some(Style::default().fg(Color::DarkGray));
}
```

(Note `●` is also the channel dot from Task 1; the channel one is `●dev`/`●beta`/standalone with prefix check that's already `starts_with('●')` — make sure exact-match comparison above runs *after* the `starts_with('●')` branch already returns. Re-order accordingly.)

- [ ] **Step 8: Build + manual**

Run: `cargo build`. Manual: with both agents echoing, watch `A● ─▶─ ▂▃▅▂▇▃▅▆ | ▁▁▂▃▅▆▇█ ─▶─ B●` — sparklines roll left as time passes, arrows briefly bold on writes.

- [ ] **Step 9: Commit**

```bash
git add src/native/footer.rs src/native/runtime.rs src/native/render.rs
git commit -m "Add traffic counters, direction arrows, sparklines, activity dots"
```

---

## Task 4: Error toast fade (item 8)

**Files:**
- Modify: `src/native/footer.rs` (fade helper + test)
- Modify: `src/native/runtime.rs` (error timestamp + fade application)

- [ ] **Step 1: Add helper + test**

Append to `footer.rs`:

```rust
pub(crate) fn error_toast_fade(msg: &str, elapsed: Duration) -> Option<String> {
    let ms = elapsed.as_millis();
    if ms >= 4_000 {
        return None; // signals "expire to default footer"
    }
    let glyph = match ms {
        0..=999 => '█',
        1_000..=1_999 => '▓',
        2_000..=2_999 => '▒',
        _ => '░',
    };
    Some(format!("{glyph} {msg}"))
}
```

Test:

```rust
#[test]
fn error_toast_fades_then_expires() {
    let m = "boom";
    assert!(error_toast_fade(m, Duration::from_millis(0)).unwrap().starts_with('█'));
    assert!(error_toast_fade(m, Duration::from_millis(1500)).unwrap().starts_with('▓'));
    assert!(error_toast_fade(m, Duration::from_millis(3500)).unwrap().starts_with('░'));
    assert!(error_toast_fade(m, Duration::from_millis(4500)).is_none());
}
```

- [ ] **Step 2: Run new tests**

Run: `cargo test -p cduo native::footer`
Expected: 13 passed.

- [ ] **Step 3: Track error timestamps in runtime**

In `src/native/runtime.rs`, add `let mut error_set_at: Option<Instant> = None;`. Whenever `footer_msg` is assigned via `write_error_footer(...)`, also do `error_set_at = Some(Instant::now());`. Anywhere else `footer_msg` is reset (default, broadcast, pause, route), set `error_set_at = None;`.

- [ ] **Step 4: Apply fade before drawing**

Just before constructing the relay-status footer:

```rust
if let Some(at) = error_set_at {
    match crate::native::footer::error_toast_fade(&footer_msg, at.elapsed()) {
        Some(faded) => footer_msg = faded,
        None => {
            footer_msg = default_footer_msg.clone();
            error_set_at = None;
        }
    }
}
```

Extend `needs_tick`: `|| error_set_at.is_some()`.

- [ ] **Step 5: Style fade glyphs DarkGray (already default — verify)**

No change required; fade glyphs are not in any styled token branch, so they inherit `DarkGray` base.

- [ ] **Step 6: Build + manual**

Run: `cargo build`. Manual: simulate write error by killing a pane child during heavy A→B traffic; the error footer should fade `█→▓→▒→░` over ~4s then snap back to the default keybinding line.

- [ ] **Step 7: Commit**

```bash
git add src/native/footer.rs src/native/runtime.rs
git commit -m "Add error toast fade for transient footer messages"
```

---

## Task 5: Hook ping dot (item 12)

**Files:**
- Modify: `src/hook.rs` (emit a "hook ping" event via mpsc — or expose a last-hit Instant)
- Modify: `src/native/runtime.rs` (subscribe + pass to footer)
- Modify: `src/native/footer.rs` (hook_ping_glyph + test)

- [ ] **Step 1: Add helper + test**

Append to `footer.rs`:

```rust
pub(crate) fn hook_ping_glyph(since_last: Option<Duration>) -> &'static str {
    match since_last {
        Some(d) if d < Duration::from_millis(400) => "·",
        Some(d) if d < Duration::from_secs(10) => " ",
        _ => "?",
    }
}
```

Test:

```rust
#[test]
fn hook_ping_glyph_phases() {
    assert_eq!(hook_ping_glyph(None), "?");
    assert_eq!(hook_ping_glyph(Some(Duration::from_millis(100))), "·");
    assert_eq!(hook_ping_glyph(Some(Duration::from_secs(3))), " ");
    assert_eq!(hook_ping_glyph(Some(Duration::from_secs(60))), "?");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p cduo native::footer`
Expected: 14 passed.

- [ ] **Step 3: Plumb hook activity into runtime**

Inspect `src/hook.rs` to find where incoming hook requests are handled. Add a `mpsc::Sender<()>` argument that fires on every request. In `runtime.rs`, create a `(hook_ping_tx, hook_ping_rx)` channel, pass `hook_ping_tx` through `RuntimeChannels`, and in the main loop drain `hook_ping_rx` into `let mut last_hook_at: Option<Instant>`.

- [ ] **Step 4: Render the dot next to the `hook:PORT` token**

Modify `default_footer_msg` construction:

```rust
let dot = crate::native::footer::hook_ping_glyph(last_hook_at.map(|t| t.elapsed()));
let mut footer_msg = format!(
    " hook:{}{dot}  · Ctrl-Y: broadcast  · …",
    hook_port,
);
```

Note: rebuild `default_footer_msg` each frame (move it inside the loop body, before the draw). Extend `needs_tick`: `|| last_hook_at.is_some()`.

- [ ] **Step 5: Build + manual**

Run: `cargo build`. Manual: trigger a hook (e.g., via a tool call from a connected agent) → `hook:8421·` flashes for ~400ms, then becomes `hook:8421 `, then `hook:8421?` after 10s of silence.

- [ ] **Step 6: Commit**

```bash
git add src/hook.rs src/native/runtime.rs src/native/footer.rs
git commit -m "Surface hook activity as inline ping dot in default footer"
```

---

## Task 6: Log ticker + divider focus tint (items 9, 15)

**Files:**
- Modify: `src/native/input.rs` (add `Ctrl-T` for ticker toggle, or similar — verify mapping convention first)
- Modify: `src/native/runtime.rs` (ticker state machine)
- Modify: `src/native/render.rs` (divider tint based on focus)
- Modify: `src/native/footer.rs` (marquee_window + test)

- [ ] **Step 1: Add marquee helper + test**

Append to `footer.rs`:

```rust
pub(crate) fn marquee_window(line: &str, width: usize, offset: usize) -> String {
    if line.chars().count() <= width || width == 0 {
        return line.to_string();
    }
    let padded: String = format!("{line}     ");
    let total = padded.chars().count();
    let start = offset % total;
    padded.chars().cycle().skip(start).take(width).collect()
}
```

Test:

```rust
#[test]
fn marquee_window_scrolls() {
    let line = "abcdef";
    assert_eq!(marquee_window(line, 10, 0), "abcdef"); // shorter than width
    let w = marquee_window(line, 4, 0);
    assert_eq!(w.chars().count(), 4);
    let w2 = marquee_window(line, 4, 1);
    assert_ne!(w, w2);
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p cduo native::footer`
Expected: 15 passed.

- [ ] **Step 3: Add `Ctrl-T` (ticker toggle) action**

Inspect `src/native/input.rs` for the existing `classify_key` function. Add a new variant to whatever action enum exists (search `GlobalAction` or similar). Map `Ctrl-T` to `ToggleLogTicker`.

- [ ] **Step 4: Wire ticker state in runtime**

Add to runtime locals:

```rust
let mut log_ticker_on = false;
let mut log_ticker_offset: usize = 0;
let mut log_ticker_last_tick = Instant::now();
```

In the main loop, before drawing, if `log_ticker_on`:

```rust
if log_ticker_last_tick.elapsed() >= Duration::from_millis(150) {
    log_ticker_offset = log_ticker_offset.wrapping_add(1);
    log_ticker_last_tick = Instant::now();
    dirty = true;
}
let log_line = std::fs::read_to_string(log_path)
    .ok()
    .and_then(|c| c.lines().last().map(str::to_string))
    .unwrap_or_default();
let area_width = 80usize; // best-effort; ratatui frame width not yet known here
footer_msg = format!(
    " log: {} ",
    crate::native::footer::marquee_window(&log_line, area_width.saturating_sub(8), log_ticker_offset),
);
```

In the key handler, toggle `log_ticker_on` on `ToggleLogTicker`.

Extend `needs_tick`: `|| log_ticker_on`.

- [ ] **Step 5: Divider focus tint**

In `src/native/render.rs`, modify `render_divider` to accept `focus: Focus` and `split: SplitLayout`. Color the divider's edge closer to the focused pane with `Color::Yellow`, the far edge `DarkGray`. Pseudocode (adapt to actual divider API):

```rust
let near_color = Color::Yellow;
let far_color = Color::DarkGray;
let (left_color, right_color) = match (split, focus.0) {
    (SplitLayout::Horizontal, PaneId::A) => (near_color, far_color),
    (SplitLayout::Horizontal, PaneId::B) => (far_color, near_color),
    (SplitLayout::Vertical, PaneId::A) => (near_color, far_color),
    (SplitLayout::Vertical, PaneId::B) => (far_color, near_color),
};
// render the divider with a gradient or split-color block
```

If the divider is a single column/row, just color it `Color::Yellow` when focus state changes side. Pass focus through `draw()`.

- [ ] **Step 6: Build + manual**

Run: `cargo build`. Manual: press Ctrl-T → footer becomes scrolling log of last entry; press again → reverts. Move focus with Ctrl-W → divider shifts color toward the focused side.

- [ ] **Step 7: Commit**

```bash
git add src/native/input.rs src/native/runtime.rs src/native/render.rs src/native/footer.rs
git commit -m "Add log ticker (Ctrl-T) and focus-tinted pane divider"
```

---

## Task 7: Final sweep — clippy + full test run

- [ ] **Step 1: Run clippy strict**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: all green; new footer module shows ≥15 tests.

- [ ] **Step 3: Manual end-to-end**

Launch `cduo` with two real agents. Exercise: pause, unpause, broadcast, route toggle, kill pane (STOP), trigger hook, toggle log ticker, switch focus. Verify each visual element from items 1–15 is observable.

- [ ] **Step 4: Commit clippy fixes if any**

```bash
git commit -am "Clippy and final polish"
```
