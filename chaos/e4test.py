#!/usr/bin/env /home/iam/venv/bin/python
"""E4 adversarial checks on one node (the script plays phones): signatures, sender/recipient/kind/time binding, replay across a node
restart, a damaged replay ledger, signing-key pinning, and the node's mail queue semantics (cumulative ACK, head-of-line).
Needs the `cryptography` package (use /home/iam/venv/bin/python). Data dirs are the harness's own (never the owner's)."""
import sys, os, time, json, ssl, socket, struct, base64, http.client
sys.path.insert(0, os.path.dirname(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "tests"))
import chaos as C
import mobile_e4_reference as E4
from cryptography.hazmat.primitives.asymmetric import x25519, ed25519

PORT = 26960
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/e4test"
binary = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else os.path.abspath("target/release/yandi")
os.makedirs(out, exist_ok=True)
os.environ["YANDI_MOBILE_TLS_PORT"] = str(PORT)
fails = []

def check(name, cond, extra=""):
    print(("ok   " if cond else "FAIL ") + name + (" " + str(extra) if extra and not cond else ""))
    if not cond:
        fails.append(name)

def ctx():
    c = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    c.check_hostname = False
    c.verify_mode = ssl.CERT_NONE
    return c

def req(method, path, body=None, token=None):
    h = http.client.HTTPSConnection("127.0.0.1", PORT, context=ctx(), timeout=15)
    hd = {"Content-Type": "application/json"}
    if token:
        hd["Authorization"] = "Bearer " + token
    h.request(method, path, json.dumps(body) if body is not None else None, hd)
    r = h.getresponse()
    raw = r.read()
    try:
        return r.status, json.loads(raw)
    except Exception:
        return r.status, raw

def ws_open(token):
    s = ctx().wrap_socket(socket.create_connection(("127.0.0.1", PORT), timeout=15))
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET /mobile/ws HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        buf += s.recv(1)
    return s

def ws_send(s, data):
    mask = os.urandom(4)
    n = len(data)
    hdr = bytes([0x82]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n))
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

def ws_recv(s, timeout=3):
    s.settimeout(timeout)
    def rd(n):
        b = b""
        while len(b) < n:
            c = s.recv(n - len(b))
            if not c:
                raise EOFError
            b += c
        return b
    h = rd(2)
    n = h[1] & 0x7F
    if n == 126:
        n = struct.unpack(">H", rd(2))[0]
    return h[0] & 0x0F, rd(n)

def frame(kind, to_hex, blob):
    return bytes([kind]) + bytes.fromhex(to_hex) + struct.pack("<I", len(blob)) + blob

def pair_text(t):
    t = t.strip()
    if t.startswith("YANDI-PAIR-1:"):
        t = base64.b64decode(t[len("YANDI-PAIR-1:"):]).decode()
    return json.loads(t)

a = C.Node(1, out, binary, anchor=True)
a.start()
assert a.login(120) and a.ready(120), a.k

def pair_phone(name):
    st, q = a.api("GET", "/api/mobile/pairing?host=127.0.0.1")
    code = pair_text(q["qr_text"])["pairing_code"]
    st, r = req("POST", "/mobile/pair", {"pairing_code": code, "device_name": name})
    token = r["token"]
    st, info = req("GET", "/mobile/info", token=token)
    return token, info["device_id"], info["node_id"]

class Phone:
    def __init__(self, name):
        self.token, self.id, self.node = pair_phone(name)
        self.x = x25519.X25519PrivateKey.generate()
        self.ed = ed25519.Ed25519PrivateKey.generate()
    def register(self, ed=None):
        ed = ed or self.ed
        xp, ep = E4.raw_pub(self.x), E4.raw_pub(ed)
        sig = ed.sign(b"YANDI-MOBILE-KEYS-V1\0" + ep + xp)
        return req("POST", "/mobile/pubkeys", {"ed25519_pub": base64.b64encode(ep).decode(), "x25519_pub": base64.b64encode(xp).decode(), "signature": base64.b64encode(sig).decode()}, token=self.token)[0]

p1, p2, p3 = Phone("p1"), Phone("p2"), Phone("p3")
node_id = p1.node
check("phones register signed bundles", p1.register() == 200 and p2.register() == 200)
st, nb = req("GET", f"/mobile/pubkey/{node_id}", token=p1.token)
node_x = x25519.X25519PublicKey.from_public_bytes(base64.b64decode(nb["x25519_pub"]))
node_ed = base64.b64decode(nb["ed25519_pub"])
check("the node publishes its own signed bundle", st == 200 and len(node_ed) == 32)

def history_texts(phone):
    st, h = a.api("GET", f"/api/chat/history/{phone.id}")
    return [m["text"] for m in h.get("messages", [])] if st == 200 and isinstance(h, dict) else []

def send_to_node(phone, text, ws=None, **kw):
    kw.setdefault("signer", phone.ed)
    kw.setdefault("sender", phone.id)
    kw.setdefault("recipient", node_id)
    signer, sender, recipient = kw.pop("signer"), kw.pop("sender"), kw.pop("recipient")
    blob = E4.seal(text, node_x, signer, sender, recipient, **kw)
    s = ws or ws_open(phone.token)
    ws_send(s, frame(0x30, node_id, blob))
    time.sleep(0.7)
    return blob, s

s1 = ws_open(p1.token)
blob, _ = send_to_node(p1, "hello E4", ws=s1)
check("a valid E4 text from a registered phone reaches the node's history", "hello E4" in history_texts(p1), history_texts(p1))
ws_send(s1, frame(0x30, node_id, blob)); time.sleep(0.7)
check("the same blob again is not shown twice", history_texts(p1).count("hello E4") == 1)

def rejected(label, text, **kw):
    send_to_node(p1, text, ws=s1, **kw)
    check(label, text not in history_texts(p1), history_texts(p1))

rejected("sender field of another device is refused", "bad-from", sender=p2.id)
rejected("recipient field of another party is refused", "bad-to", recipient=p2.id)
rejected("kind=2 (live) cannot be played into the chat channel", "bad-kind", kind=2)
rejected("a signature by an unregistered key is refused", "bad-signer", signer=ed25519.Ed25519PrivateKey.generate())
rejected("a timestamp 10 minutes ahead is refused", "future", timestamp=int(time.time() * 1000) + 600_000)
rejected("a timestamp 31 days old is refused", "old", timestamp=int((time.time() - 31 * 86400) * 1000))
send_to_node(p1, "four-min-ahead", ws=s1, timestamp=int(time.time() * 1000) + 240_000)
check("a timestamp 4 minutes ahead (clock skew) is accepted", "four-min-ahead" in history_texts(p1))

good = E4.seal("tamper-me", node_x, p1.ed, p1.id, node_id)
for pos, label in [(1, "kind byte"), (5, "sender bytes"), (84, "timestamp"), (100, "ephemeral key"), (125, "nonce"), (140, "ciphertext"), (len(good) - 70, "tag"), (len(good) - 3, "signature")]:
    t = bytearray(good); t[pos] ^= 1
    ws_send(s1, frame(0x30, node_id, bytes(t))); time.sleep(0.4)
check("flipping any byte (kind/sender/time/ephemeral/nonce/ciphertext/tag/signature) is refused", "tamper-me" not in history_texts(p1))
ws_send(s1, frame(0x30, node_id, good)); time.sleep(0.5)
check("the untouched blob is still accepted afterwards (refusals do not burn ids)", "tamper-me" in history_texts(p1))

# service envelopes the node does not know (avatars between phones) must not show up as chat messages
for svc_text in ['\x01yandi-avh:{"h":"' + "ab" * 32 + '"}', '\x01yandi-avq:{"h":"' + "cd" * 32 + '"}', '\x01yandi-newthing:{}']:
    send_to_node(p1, svc_text, ws=s1)
check("unknown service envelopes (avatars) are dropped, not shown as messages", not any("yandi-" in t for t in history_texts(p1)), history_texts(p1))

# kind separation on the live channel: an ordinary kind=1 blob sent as a live signal is refused (the node answers "peer offline" to the sender)
call = '\x01yandi-call:' + json.dumps({"v": 1, "t": "invite", "cid": "abcdef0123456789", "video": False, "ts": int(time.time() * 1000)})
a.api("GET", "/api/mobile/call/events?after=0")   # the page is "open" from now on
k1 = E4.seal(call, node_x, p1.ed, p1.id, node_id, kind=1)
ws_send(s1, frame(0x31, node_id, k1)); time.sleep(0.7)
st, ev = a.api("GET", "/api/mobile/call/events?after=0")
check("a kind=1 blob cannot be replayed as a call signal", not ev.get("events"), ev)
k2 = E4.seal(call, node_x, p1.ed, p1.id, node_id, kind=2)
ws_send(s1, frame(0x31, node_id, k2)); time.sleep(0.7)
st, ev = a.api("GET", "/api/mobile/call/events?after=0")
check("a proper kind=2 call signal reaches the page", len(ev.get("events", [])) == 1, ev)
ws_send(s1, frame(0x31, node_id, k2)); time.sleep(0.7)
st, ev = a.api("GET", "/api/mobile/call/events?after=0")
check("the call signal is not delivered twice", len(ev.get("events", [])) == 1, ev)

# signing-key pinning on the node
check("a new signing key under the same token is refused (409)", p1.register(ed25519.Ed25519PrivateKey.generate()) == 409)
p1.x = x25519.X25519PrivateKey.generate()
check("a new X25519 key under the same signing key is accepted", p1.register() == 200)

# replay across a node restart
kept = E4.seal("survives-restart", node_x, p1.ed, p1.id, node_id)
ws_send(s1, frame(0x30, node_id, kept)); time.sleep(0.6)
check("message before restart is shown", "survives-restart" in history_texts(p1))
s1.close()
a.kill(); time.sleep(2); a.start()
assert a.login(120) and a.ready(120)
time.sleep(3)
s1 = ws_open(p1.token)
before = history_texts(p1).count("survives-restart")
ws_send(s1, frame(0x30, node_id, kept)); time.sleep(0.8)
check("the same blob after a node restart is still refused (ledger on disk)", history_texts(p1).count("survives-restart") == before == 1, (before, history_texts(p1)))
st, nb2 = req("GET", f"/mobile/pubkey/{node_id}", token=p1.token)
check("the node keeps its identity keys across a restart", nb2.get("ed25519_pub") == nb["ed25519_pub"])

# damaged ledger: fail closed, then recover
ledger = os.path.join(a.dir, "home/.local/share/yandi/mobile_replay_e4.json")
check("the replay ledger exists on disk", os.path.exists(ledger) and os.path.getsize(ledger) > 10, ledger)
good_ledger = open(ledger, "rb").read()
open(ledger, "wb").write(b"{not json")
send_to_node(p1, "while-ledger-corrupt", ws=s1)
check("a damaged ledger fails closed (nothing is accepted, nothing is reset)", "while-ledger-corrupt" not in history_texts(p1) and open(ledger, "rb").read() == b"{not json")
open(ledger, "wb").write(good_ledger)
send_to_node(p1, "after-ledger-restored", ws=s1)
check("after the ledger is restored messages flow again", "after-ledger-restored" in history_texts(p1))

# mail queue semantics (phone -> phone relay): cumulative ACK and head-of-line
s_a = ws_open(p1.token)
for i in range(3):
    ws_send(s_a, frame(0x30, p2.id, b"\x01legacy-or-garbage-%d" % i + os.urandom(150)))
    time.sleep(0.15)
ws_send(s_a, frame(0x30, p2.id, E4.seal("valid-mail", p2.x.public_key(), p1.ed, p1.id, p2.id)))
time.sleep(0.5)
st, r = req("GET", "/mobile/inbox", token=p2.token)
msgs = r["messages"]
check("p2's queue holds the 3 undecryptable mails and the valid one", len(msgs) == 4, len(msgs))
last = msgs[-1]["id"]
req("POST", "/mobile/inbox/ack", {"ids": [last]}, token=p2.token)
st, r = req("GET", "/mobile/inbox", token=p2.token)
print("INFO cumulative ACK: after acknowledging only the newest mail the queue holds", len(r["messages"]), "of 4 (the node drops everything older than the ACKed id)")
check("[documented] an ACK of a later mail also removes earlier unACKed mail on the node", len(r["messages"]) == 0)

for i in range(205):
    ws_send(s_a, frame(0x30, p3.id, b"\x01stuck-%d" % i + os.urandom(150)))
ws_send(s_a, frame(0x30, p3.id, E4.seal("behind-the-wall", p3.x.public_key(), p1.ed, p1.id, p3.id)))
time.sleep(2.5)
st, r = req("GET", "/mobile/inbox", token=p3.token)
print("INFO head-of-line: /mobile/inbox returned", len(r["messages"]), "mails; the valid one is among them:",
      any(base64.b64decode(m["payload_b64"])[:2] == bytes([0xE4, 1]) for m in r["messages"]))
check("[finding] the oldest 200 unACKed mails hide newer ones (valid mail beyond 200 is not in the first page)",
      not any(base64.b64decode(m["payload_b64"])[:2] == bytes([0xE4, 1]) for m in r["messages"]))

# last activity in the contact list and big file chunks
st, r = req("GET", "/mobile/contacts", token=p1.token)
devs = [c for c in r["contacts"] if c["peer_id"] in (p2.id, p3.id)]
check("every paired device carries last_activity_ms in /mobile/contacts", len(devs) == 2 and all(isinstance(c.get("last_activity_ms"), int) and c["last_activity_ms"] > 0 for c in devs), devs)
size = 6 * 1024 * 1024
st, r = req("POST", "/mobile/files", {"to_peer_id": p2.id, "file_name": "x", "file_size": size, "total_chunks": 6}, token=p1.token)
tid = r.get("transfer_id")
def put(idx, n):
    h = http.client.HTTPSConnection("127.0.0.1", PORT, context=ctx(), timeout=30)
    h.request("PUT", f"/mobile/files/{tid}/chunk/{idx}", os.urandom(n), {"Authorization": "Bearer " + p1.token, "Content-Type": "application/octet-stream"})
    r = h.getresponse(); r.read(); return r.status
check("a 1 MiB chunk (big-file mode) is accepted", st == 200 and put(0, (1 << 20) + 28) == 200, (st, r))
check("a chunk above the new limit is refused", put(1, (1 << 20) + 2048) in (400, 413))
req("DELETE", f"/mobile/files/{tid}", token=p1.token)

a.kill()
print("FAILED:" if fails else "ALL OK", fails or "")
sys.exit(1 if fails else 0)
