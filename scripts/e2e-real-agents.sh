#!/bin/bash
set -euo pipefail

echo "=== cduo real agent relay E2E ==="

if ! command -v python3 >/dev/null 2>&1; then
    echo "FAIL: python3 is required"
    exit 1
fi

if ! command -v tmux >/dev/null 2>&1; then
    echo "FAIL: tmux is required"
    exit 1
fi

if ! command -v claude >/dev/null 2>&1; then
    echo "FAIL: claude CLI is not on PATH"
    exit 1
fi

if ! command -v codex >/dev/null 2>&1; then
    echo "FAIL: codex CLI is not on PATH"
    exit 1
fi

echo "Building release binary..."
cargo build --release >/dev/null

python3 - << 'PY'
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path.cwd()
BINARY = ROOT / "target" / "release" / "cduo"


def run(cmd, *, cwd, env, timeout=30, check=True):
    result = subprocess.run(
        cmd,
        cwd=str(cwd),
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )
    if check and result.returncode != 0:
        raise RuntimeError(f"command failed: {' '.join(map(str, cmd))}\n{result.stdout}")
    return result.stdout


def capture(session_name: str, pane: int) -> str:
    result = subprocess.run(
        ["tmux", "capture-pane", "-pt", f"{session_name}:0.{pane}", "-S", "-300"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return result.stdout


def wait_for_panes(session_name: str, timeout: float = 30.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = subprocess.run(
            ["tmux", "list-panes", "-t", session_name, "-F", "#{pane_index} #{pane_current_command}"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if result.returncode == 0 and len(result.stdout.strip().splitlines()) >= 2:
            return
        time.sleep(0.2)
    raise RuntimeError(f"tmux panes not ready for {session_name}")


def wait_capture(session_name: str, pane: int, needle: str, timeout: float, label: str) -> str:
    deadline = time.time() + timeout
    last = ""
    while time.time() < deadline:
        last = capture(session_name, pane)
        if needle in last:
            return last
        time.sleep(1.0)
    raise AssertionError(
        f"timed out waiting for {needle!r} in {label}; pane tail={last[-2000:]!r}"
    )


def wait_capture_regex(session_name: str, pane: int, pattern: str, timeout: float, label: str) -> str:
    deadline = time.time() + timeout
    last = ""
    compiled = re.compile(pattern, flags=re.MULTILINE)
    while time.time() < deadline:
        last = capture(session_name, pane)
        if compiled.search(last):
            return last
        time.sleep(1.0)
    raise AssertionError(
        f"timed out waiting for regex {pattern!r} in {label}; pane tail={last[-2000:]!r}"
    )


def send_prompt(session_name: str, pane: int, prompt: str) -> None:
    session_id = re.sub(r"^.*-(\d+)$", r"cduo-\1", session_name)
    pane_id = "a" if pane == 0 else "b"
    socket_path = f"/tmp/cduo-{session_id}-{pane_id}.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.connect(socket_path)
        sock.sendall(prompt.encode())
        time.sleep(0.3)
        sock.sendall(b"\r\n")


def start_agent(agent: str, work_dir: Path, env: dict) -> str:
    output = run([str(BINARY), agent, "--new"], cwd=work_dir, env=env, timeout=60)
    match = re.search(r"^Session:\s+(\S+)", output, flags=re.MULTILINE)
    if not match:
        raise RuntimeError(f"could not parse session name from cduo output:\n{output}")
    return match.group(1)


def stop_agent(session_name: str, work_dir: Path, env: dict) -> None:
    run([str(BINARY), "stop", session_name], cwd=work_dir, env=env, timeout=30, check=False)
    subprocess.run(["tmux", "kill-session", "-t", session_name], stderr=subprocess.DEVNULL)


def run_agent(agent: str) -> None:
    temp = Path(tempfile.mkdtemp(prefix=f"cduo-real-{agent}-"))
    session_name = None
    failed = False
    try:
        work_dir = ROOT
        state_dir = temp / "state"
        state_dir.mkdir(parents=True)

        env = os.environ.copy()
        env["CDUO_STATE_DIR"] = str(state_dir)
        env["PATH"] = f"{BINARY.parent}{os.pathsep}{env.get('PATH', '')}"

        session_name = start_agent(agent, work_dir, env)
        wait_for_panes(session_name)
        time.sleep(8.0)

        pane_a_start = capture(session_name, 0)
        pane_b_start = capture(session_name, 1)
        if "Yes, I trust this folder" in pane_a_start:
            subprocess.run(["tmux", "send-keys", "-t", f"{session_name}:0.0", "Enter"], check=True)
        if "Yes, I trust this folder" in pane_b_start:
            subprocess.run(["tmux", "send-keys", "-t", f"{session_name}:0.1", "Enter"], check=True)
        if "Yes, continue" in pane_a_start:
            subprocess.run(["tmux", "send-keys", "-t", f"{session_name}:0.0", "Enter"], check=True)
        if "Yes, continue" in pane_b_start:
            subprocess.run(["tmux", "send-keys", "-t", f"{session_name}:0.1", "Enter"], check=True)

        if agent == "claude":
            wait_capture(session_name, 0, "❯", 120.0, "claude pane A prompt")
            wait_capture(session_name, 1, "❯", 120.0, "claude pane B prompt")
        else:
            wait_capture(session_name, 0, "Context", 180.0, "codex pane A prompt")
            wait_capture(session_name, 1, "Context", 180.0, "codex pane B prompt")

        final_marker = f"CDUO_REAL_SUBMIT_{agent.upper()}_{os.getpid()}"
        marker_words = " ".join(final_marker.split("_"))
        prompt = f"Reply exactly with the words {marker_words} joined by underscores and nothing else."
        assistant_line = r"⏺.*" if agent == "claude" else r"•.*"

        send_prompt(session_name, 0, prompt)
        wait_capture_regex(
            session_name,
            0,
            assistant_line + re.escape(final_marker),
            180.0,
            f"{agent} pane A response",
        )
        wait_capture_regex(
            session_name,
            1,
            r"(?s)" + re.escape(final_marker) + r".*" + assistant_line,
            180.0,
            f"{agent} pane B submitted response",
        )
        print(f"PASS: {agent} real response relayed and submitted from pane A to pane B")
    except Exception:
        failed = True
        if session_name:
            print(f"--- {agent} pane A tail ---")
            print(capture(session_name, 0)[-2500:])
            print(f"--- {agent} pane B tail ---")
            print(capture(session_name, 1)[-2500:])
            log_path = temp / "state" / "sessions" / re.sub(r"^.*-(\d+)$", r"cduo-\1", session_name) / "daemon.log"
            if log_path.exists():
                print(f"--- {agent} daemon.log ---")
                print(log_path.read_text()[-4000:])
            else:
                print(f"--- {agent} daemon.log missing: {log_path} ---")
        raise
    finally:
        if session_name:
            stop_agent(session_name, ROOT, os.environ.copy() | {"CDUO_STATE_DIR": str(temp / "state")})
        if failed:
            print(f"Preserved failed test directory: {temp}")
        else:
            shutil.rmtree(temp, ignore_errors=True)


agent_names = os.environ.get("CDUO_E2E_AGENTS", "claude,codex").split(",")
for agent_name in [name.strip() for name in agent_names if name.strip()]:
    run_agent(agent_name)

print("=== real agent relay E2E PASSED ===")
PY
