#!/bin/bash
set -euo pipefail

# cduo v2 E2E Test
# Tests daemon lifecycle with mock agents

echo "=== cduo v2 E2E Test ==="

BINARY=""
TEST_DIR=""
SESSION_ID=""
DAEMON_PID=""

cleanup() {
    if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        sleep 0.2
        kill -9 "$DAEMON_PID" 2>/dev/null || true
    fi

    if [[ -n "${SESSION_ID:-}" ]]; then
        rm -f "/tmp/cduo-$SESSION_ID.sock" "/tmp/cduo-$SESSION_ID-a.sock" "/tmp/cduo-$SESSION_ID-b.sock" 2>/dev/null || true
    fi

    if [[ -n "${TEST_DIR:-}" ]]; then
        rm -rf "$TEST_DIR"
    fi
}
trap cleanup EXIT INT TERM

# Build release binary first
echo "Building release binary..."
cargo build --release 2>/dev/null

BINARY="$(pwd)/target/release/cduo"
TEST_DIR="$(mktemp -d)"
MOCK_BIN="$TEST_DIR/bin"
SESSION_DIR="$TEST_DIR/sessions"

mkdir -p "$MOCK_BIN"
mkdir -p "$SESSION_DIR"

# Create mock claude binary
cat > "$MOCK_BIN/claude" << 'EOF'
#!/bin/bash
echo "Mock Claude started"
echo "TERMINAL_ID=$TERMINAL_ID"
echo "$ "
while IFS= read -r line; do
    if [[ "$line" == Relay\ from* ]]; then
        echo "$ "
        continue
    fi
    echo "⏺ Relay from $TERMINAL_ID: $line"
    echo "$ "
done
EOF
chmod +x "$MOCK_BIN/claude"

# Create session metadata
SESSION_ID="cduo-test-e2e-$$"
HOOK_PORT="$(python3 - << 'EOF'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
EOF
)"
mkdir -p "$SESSION_DIR/$SESSION_ID"

cat > "$SESSION_DIR/$SESSION_ID/session.json" << EOF
{
    "session_id": "$SESSION_ID",
    "session_name": "cduo-test-e2e",
    "project_name": "test",
    "display_name": "test · claude",
    "cwd": "$TEST_DIR",
    "created_at": "2024-01-01T00:00:00Z",
    "agent": "claude",
    "mode": null,
    "hook_port": $HOOK_PORT,
    "panes": {
        "a": {"pane_id": "a", "attach_port": 0},
        "b": {"pane_id": "b", "attach_port": 0}
    }
}
EOF

export PATH="$MOCK_BIN:$PATH"
export CDUO_STATE_DIR="$TEST_DIR"

echo "Starting daemon..."
"$BINARY" __daemon --session "$SESSION_ID" &
DAEMON_PID=$!

# Wait for sockets
SOCKET="/tmp/cduo-$SESSION_ID.sock"
ATTACH_A="/tmp/cduo-$SESSION_ID-a.sock"
ATTACH_B="/tmp/cduo-$SESSION_ID-b.sock"

echo "Waiting for sockets..."
for i in {1..30}; do
    if [[ -S "$SOCKET" && -S "$ATTACH_A" && -S "$ATTACH_B" ]]; then
        break
    fi
    sleep 0.1
done

if [[ ! -S "$SOCKET" ]]; then
    echo "FAIL: Control socket not created"
    kill $DAEMON_PID 2>/dev/null || true
    exit 1
fi

echo "Sockets ready: control=$SOCKET attach_a=$ATTACH_A attach_b=$ATTACH_B"

# Test 1: Write to attach socket A and read the agent response.
echo "Test 1: Writing through attach socket A..."
python3 -c "
import socket, time, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect('$ATTACH_A')
    s.settimeout(2.0)
    s.sendall(b'startup-check\\r')
    data = b''
    for _ in range(20):
        try:
            chunk = s.recv(4096)
            if chunk:
                data += chunk
                if b'Relay from a: startup-check' in data:
                    break
        except socket.timeout:
            break
    if b'Relay from a: startup-check' in data:
        print('PASS: Attach socket A writes to and reads from mock agent')
        sys.exit(0)
    else:
        print(f'FAIL: Expected mock agent response, got: {data!r}')
        sys.exit(1)
except Exception as e:
    print(f'FAIL: {e}')
    sys.exit(1)
finally:
    s.close()
"

# Test 2: Verify Claude hook relay from pane A to pane B
echo "Test 2: Relaying pane A output to pane B..."
python3 -c "
import json
import socket
import sys
import time
import urllib.request

def connect(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(path)
    s.settimeout(3.0)
    return s

def read_until(sock, needle, timeout=5.0):
    deadline = time.time() + timeout
    data = b''
    while time.time() < deadline:
        try:
            chunk = sock.recv(4096)
            if chunk:
                data += chunk
                if needle in data:
                    return data
            else:
                time.sleep(0.05)
        except socket.timeout:
            pass
    return data

a = connect('$ATTACH_A')
b = connect('$ATTACH_B')
try:
    read_until(a, b'$ ', 3.0)
    read_until(b, b'$ ', 3.0)

    a.sendall(b'relay-check\\r')
    pane_a = read_until(a, b'Relay from a: relay-check', 5.0)
    if b'Relay from a: relay-check' not in pane_a:
        print(f'FAIL: Pane A did not produce relay source output: {pane_a!r}')
        sys.exit(1)

    multiline = 'Relay from a: relay-check\\nsecond line from relay\\nthird line from relay'
    transcript_path = '$TEST_DIR/transcript-a.jsonl'
    with open(transcript_path, 'w') as f:
        f.write(json.dumps({
            'type': 'assistant',
            'message': {
                'role': 'assistant',
                'content': [{'type': 'text', 'text': multiline}],
            },
        }) + '\\n')
        f.write(json.dumps({
            'type': 'system',
            'subtype': 'stop_hook_summary',
        }) + '\\n')

    payload = json.dumps({
        'type': 'stop',
        'terminal_id': 'a',
        'transcript_path': transcript_path,
    }).encode()
    req = urllib.request.Request(
        'http://127.0.0.1:$HOOK_PORT/hook',
        data=payload,
        headers={'Content-Type': 'application/json'},
        method='POST',
    )
    urllib.request.urlopen(req, timeout=3).read()

    pane_b = read_until(b, b'third line from relay', 5.0)
    if (
        (b'\x1b[200~Relay from a: relay-check' in pane_b or b'^[[200~Relay from a: relay-check' in pane_b)
        and b'second line from relay' in pane_b
        and (b'third line from relay\x1b[201~' in pane_b or b'third line from relay^[[201~' in pane_b)
    ):
        print('PASS: Multiline Pane A output relayed into pane B as bracketed paste')
    else:
        print(f'FAIL: Pane B did not receive exact bracketed paste relay: {pane_b!r}')
        sys.exit(1)

    transcript_b_path = '$TEST_DIR/transcript-b.jsonl'
    with open(transcript_b_path, 'w') as f:
        f.write(json.dumps({
            'type': 'assistant',
            'message': {
                'role': 'assistant',
                'content': [{'type': 'text', 'text': 'Reply from b after relay'}],
            },
        }) + '\\n')
        f.write(json.dumps({
            'type': 'system',
            'subtype': 'stop_hook_summary',
        }) + '\\n')

    payload = json.dumps({
        'type': 'stop',
        'terminal_id': 'b',
        'transcript_path': transcript_b_path,
    }).encode()
    req = urllib.request.Request(
        'http://127.0.0.1:$HOOK_PORT/hook',
        data=payload,
        headers={'Content-Type': 'application/json'},
        method='POST',
    )
    urllib.request.urlopen(req, timeout=3).read()
    time.sleep(0.5)

    log_path = '$SESSION_DIR/$SESSION_ID/daemon.log'
    with open(log_path) as f:
        log = f.read()
    if 'publish source=b target=a len=24 text=\"Reply from b after relay\"' in log:
        print('PASS: Reply produced from relayed input was relayed back')
    else:
        print('FAIL: Reply from relayed input was not published back')
        print(log)
        sys.exit(1)
finally:
    a.close()
    b.close()
"

# Test 3: Send stop command via control socket
echo "Test 3: Sending stop command..."
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect('$SOCKET')
    req = {'cmd': 'stop', 'session_id': '$SESSION_ID'}
    s.sendall(json.dumps(req).encode() + b'\n')
    resp = s.recv(4096).decode()
    print(f'Response: {resp}')
    if 'ok' in resp and 'true' in resp:
        print('PASS: Stop command accepted')
    else:
        print('FAIL: Stop command rejected')
except Exception as e:
    print(f'FAIL: {e}')
finally:
    s.close()
"

# Wait for daemon to exit
echo "Waiting for daemon shutdown..."
for i in {1..30}; do
    if ! kill -0 $DAEMON_PID 2>/dev/null; then
        break
    fi
    sleep 0.1
done

# Verify cleanup
if [[ -S "$SOCKET" || -S "$ATTACH_A" || -S "$ATTACH_B" ]]; then
    echo "FAIL: Sockets not cleaned up"
    exit 1
fi
echo "PASS: Sockets cleaned up"

# Verify PID file removed
if [[ -f "$SESSION_DIR/$SESSION_ID/daemon.pid" ]]; then
    echo "FAIL: PID file not cleaned up"
    exit 1
fi
echo "PASS: PID file cleaned up"

echo ""
echo "=== All E2E tests PASSED ==="
