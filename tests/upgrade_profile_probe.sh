#!/usr/bin/env bash
# Run inside the disposable namespace prepared by upgrade_profile_namespace.sh.
# The namespace has loopback only; it cannot reach public bootstrap services.
set -euo pipefail
probe_root=${1:?stand root required}
probe_bin=${2:?node binary required}
ip link set lo up
probe_node="$probe_root/stand/node1"
env HOME="$probe_node/home" XDG_DATA_HOME="$probe_node/home/.local/share" \
    XDG_CONFIG_HOME="$probe_node/home/.config" YANDI_CONFIG="$probe_node/config.yaml" \
    YANDI_P2P_DISCOVERY_PORT=26103 YANDI_P2P_DATA_PORT=26104 \
    YANDI_CORE_BIN=self YANDI_TESTNET=1 YANDI_LOCAL_TCP=1 \
    NO_PROXY=127.0.0.1,localhost,::1 \
    "$probe_bin" --anchor >"$probe_root/new-node.log" 2>&1 &
probe_pid=$!
trap 'kill "$probe_pid" || true; wait "$probe_pid" || true' EXIT
/usr/bin/python3 - "$probe_root/stand" <<'PY'
import base64
import json
import pathlib
import sys
import time
import urllib.error
import urllib.request

stand = pathlib.Path(sys.argv[1])
password = next(line.split(':', 1)[1].strip() for line in (stand / 'passwords.txt').read_text().splitlines() if line.startswith('пароль входа:'))
base = 'http://127.0.0.1:26100'
cookie = None
for attempt in range(120):
    try:
        request = urllib.request.Request(base + '/api/auth/login', data=json.dumps({'login_password': password}).encode(), headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=3) as response:
            cookie = response.headers['Set-Cookie'].split(';', 1)[0]
        break
    except (urllib.error.URLError, KeyError):
        time.sleep(.5)
assert cookie, 'new node did not become ready'

def get(path):
    request = urllib.request.Request(base + path, headers={'Cookie': cookie})
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)

peers = get('/api/peers/trusted')
assert len(peers['peers']) == 1, 'trusted peer disappeared'
peer = peers['peers'][0]['id']
history = get('/api/chat/history/' + peer)
texts = {message.get('text') for message in history.get('messages', [])}
assert {'upgrade-synthetic-history-1', 'upgrade-synthetic-history-2'} <= texts, 'history changed'
contacts = get('/api/contacts')
assert 'upgrade-synthetic-contact' in json.dumps(contacts), 'contact disappeared'
outbox = stand / 'node1/home/.yandi/chats/outbox.enc'
assert outbox.is_file() and outbox.stat().st_size > 0, 'pending queue disappeared'
print('new_binary_login=PASS history=PASS contacts=PASS outbox_present=PASS')
PY
