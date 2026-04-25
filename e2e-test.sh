#!/bin/bash
set -euo pipefail

# cduo v2 E2E Test
# Tests daemon lifecycle with mock agents

echo "=== cduo v2 E2E Test ==="

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
while IFS= read -r line; do
    echo "Echo: $line"
done
EOF
chmod +x "$MOCK_BIN/claude"

# Create session metadata
SESSION_ID="cduo-test-e2e-$$"
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
    "hook_port": 53333,
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

# Test 1: Read from attach socket A (should see mock claude output)
echo "Test 1: Reading PTY output from attach socket A..."
python3 -c "
import socket, time, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect('$ATTACH_A')
    s.settimeout(2.0)
    data = b''
    for _ in range(20):
        try:
            chunk = s.recv(4096)
            if chunk:
                data += chunk
                if b'Mock Claude started' in data:
                    break
        except socket.timeout:
            break
    if b'Mock Claude started' in data:
        print('PASS: Received mock agent output')
        sys.exit(0)
    else:
        print(f'FAIL: Expected mock agent output, got: {data!r}')
        sys.exit(1)
except Exception as e:
    print(f'FAIL: {e}')
    sys.exit(1)
finally:
    s.close()
"

# Test 2: Send stop command via control socket
echo "Test 2: Sending stop command..."
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

# Cleanup
rm -rf "$TEST_DIR"
