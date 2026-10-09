#!/usr/bin/env python3
"""The phone API on the TLS entry end to end: pairing by code, info, contacts, send, history, inbox/ack, and the message socket.
Two nodes on the loopback; node 1 has the TLS entry; the script plays the phone."""
import sys, os, time, json, ssl, socket, struct, hashlib, base64, http.client
sys.path.insert(0, os.path.dirname(__file__))
import chaos as C

PORT = 26900
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/mobiletest"
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
    s.sendall((f"GET /mobile/ws?token={token} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        buf += s.recv(1)
    return s, buf.split(b"\r\n")[0]

def ws_send(s, data):
    mask = os.urandom(4)
    n = len(data)
    hdr = bytes([0x82]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n))
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

def ws_recv(s, timeout=20):
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

a, b = C.Node(1, out, binary, anchor=True), C.Node(2, out, binary)
for x in (a, b):
    x.start()
for x in (a, b):
    assert x.login(120) and x.ready(120), x.k
a.trust(b); b.trust(a)
json.dump({"contacts": [{"id": "1", "name": "Bob", "short_id": b.id[:16]}]}, open(os.path.join(a.dir, "contacts.json"), "w"))
for _ in range(120):
    if a.peers_online() and b.peers_online():
        break
    time.sleep(1)
time.sleep(5)

st, _ = req("GET", "/mobile/info")
check("no token -> 401", st == 401, st)
st, _ = req("POST", "/mobile/pair", {"pairing_code": "000000"})
check("pair without an issued code refused", st == 403, st)
st, q = a.api("GET", "/api/mobile/pairing?host=127.0.0.1")
check("web issues the QR", st == 200 and q.get("status") == "ok" and "<svg" in q.get("qr_svg", ""), (st, q))
qr = json.loads(q["qr_text"])
check("QR carries host, port, fingerprint, tls", qr["port"] == PORT and qr["tls"] is True and len(qr["tls_fingerprint"]) == 64, qr)
der = ssl.get_server_certificate(("127.0.0.1", PORT))
fp = hashlib.sha256(ssl.PEM_cert_to_DER_cert(der)).hexdigest()
check("fingerprint in QR = the certificate on the wire", fp == qr["tls_fingerprint"], (fp, qr["tls_fingerprint"]))
for _ in range(5):
    st, _ = req("POST", "/mobile/pair", {"pairing_code": "999999" if qr["pairing_code"] != "999999" else "111111"})
check("wrong code refused", st == 403, st)
st, r = req("POST", "/mobile/pair", {"pairing_code": qr["pairing_code"], "device_name": "test phone"})
check("code burnt after 5 wrong tries", st == 403, st)
st, q = a.api("GET", "/api/mobile/pairing?host=127.0.0.1")
qr = json.loads(q["qr_text"])
st, r = req("POST", "/mobile/pair", {"pairing_code": qr["pairing_code"], "device_name": "test phone"})
check("pairing gives a token", st == 200 and len(r.get("token", "")) == 64, (st, r))
tok = r["token"]
st, r = req("POST", "/mobile/pair", {"pairing_code": qr["pairing_code"]})
check("code works once", st == 403, st)
st, info = req("GET", "/mobile/info", token=tok)
check("info has the full node id", st == 200 and info["node_id"] == a.id and len(info["node_id"]) == 64, (st, info))
st, r = req("GET", "/mobile/info", token="0" * 64)
check("wrong token -> 401", st == 401, st)
st, r = req("GET", "/mobile/contacts", token=tok)
c = r["contacts"] if st == 200 else []
check("contact Bob is online and has the full id", len(c) == 1 and c[0]["peer_id"] == b.id and c[0]["online"], r)
st, r = req("POST", f"/mobile/chat/{b.id}", {"text": "from phone via REST"}, token=tok)
check("REST send accepted", st == 200, (st, r))
time.sleep(4)
st, h = b.api("GET", f"/api/chat/history/{a.id}")
check("Bob received it", any(m["text"] == "from phone via REST" for m in h.get("messages", [])), h)
st, r = req("GET", f"/mobile/chat/{b.id}?limit=10", token=tok)
check("history shows it", st == 200 and any(m["text"] == "from phone via REST" for m in r["messages"]), r)
# Bob writes to the phone's node while the phone is offline -> inbox
b.api("POST", f"/api/chat/send/{a.id}", {"text": "offline hello"})
time.sleep(4)
st, r = req("GET", "/mobile/inbox?since=0&limit=50", token=tok)
msgs = r.get("messages", []) if st == 200 else []
dec = [base64.b64decode(m["payload_b64"]).decode() for m in msgs]
check("inbox has the message from Bob", "offline hello" in dec, (st, dec))
if msgs:
    st, _ = req("POST", "/mobile/inbox/ack", {"ids": [m["id"] for m in msgs]}, token=tok)
    st, r = req("GET", "/mobile/inbox?since=0", token=tok)
    check("after ack the inbox is empty", st == 200 and r["messages"] == [], r)
# message socket
s, line = ws_open(tok)
check("socket upgrades", b"101" in line, line)
ws_send(s, bytes([0x01]))
op, d = ws_recv(s)
check("ping -> pong", d[:1] == bytes([0x02]), d)
b.api("POST", f"/api/chat/send/{a.id}", {"text": "live hello"})
got = None
try:
    for _ in range(5):
        op, d = ws_recv(s, 20)
        if d[:1] == bytes([0x10]):
            got = d
            break
except Exception as e:
    got = None
ok = got is not None and got[1:33] == bytes.fromhex(b.id) and got[45:45 + struct.unpack("<I", got[41:45])[0]] == b"live hello"
check("live message arrives as a chat frame", ok, got)
payload = b"sent by socket"
ws_send(s, bytes([0x30]) + bytes.fromhex(b.id) + struct.pack("<I", len(payload)) + payload)
time.sleep(5)
st, h = b.api("GET", f"/api/chat/history/{a.id}")
check("Bob received the socket message", any(m["text"] == "sent by socket" for m in h.get("messages", [])), h)
# ---- two phones of the owner through the PC
def pair_phone(name):
    st, q = a.api("GET", "/api/mobile/pairing?host=127.0.0.1")
    code = json.loads(q["qr_text"])["pairing_code"]
    st, r = req("POST", "/mobile/pair", {"pairing_code": code, "device_name": name})
    return r["token"]
tok1 = tok
tok2 = pair_phone("YANDI Mobile")
st, i1 = req("GET", "/mobile/info", token=tok1)
st, i2 = req("GET", "/mobile/info", token=tok2)
d1, d2 = i1["device_id"], i2["device_id"]
check("each phone has its own id", d1 and d2 and d1 != d2 and len(d1) == 64, (d1, d2))
st, c1 = req("GET", "/mobile/contacts", token=tok1)
other = [x for x in c1["contacts"] if x["peer_id"] == d2]
check("phone 1 sees phone 2 as a contact (offline, distinct name)", len(other) == 1 and not other[0]["online"] and "YANDI Mobile" in other[0]["display_name"], c1)
check("a phone does not see itself", not any(x["peer_id"] == d1 for x in c1["contacts"]))
kx = base64.b64encode(os.urandom(32)).decode(); ke = base64.b64encode(os.urandom(32)).decode()
st, _ = req("POST", "/mobile/pubkeys", {"ed25519_pub": ke, "x25519_pub": kx}, token=tok2)
st, r = req("GET", f"/mobile/pubkey/{d2}", token=tok1)
check("phone 1 can fetch phone 2's key", st == 200 and r["x25519_pub"] == kx, (st, r))
st, _ = req("GET", f"/mobile/pubkey/{b.id}", token=tok1)
check("an ordinary contact has no key (plain text over TLS)", st == 404, st)
# offline mail: phone 2 is not connected
blob = b"\x01opaque-e2e-blob-from-phone-1"
s1, _l = ws_open(tok1)
ws_send(s1, bytes([0x30]) + bytes.fromhex(d2) + struct.pack("<I", len(blob)) + blob)
time.sleep(1)
st, r = req("GET", "/mobile/inbox", token=tok2)
m = [x for x in r.get("messages", []) if x["from_peer_id"] == d1]
check("mail waits in phone 2's inbox, bytes untouched", len(m) == 1 and base64.b64decode(m[0]["payload_b64"]) == blob, r)
check("phone 1's inbox does not get it", not [x for x in req("GET", "/mobile/inbox", token=tok1)[1].get("messages", []) if x["from_peer_id"] == d1])
# live: phone 2 connects, presence + live message
s2, _l = ws_open(tok2)
op, d = ws_recv(s1, 10)
check("phone 1 is told phone 2 came online", d[:1] == bytes([0x12]) and d[1:33] == bytes.fromhex(d2) and d[33] == 1, d)
blob2 = b"live blob"
ws_send(s1, bytes([0x30]) + bytes.fromhex(d2) + struct.pack("<I", len(blob2)) + blob2)
got = None
for _ in range(5):
    op, d = ws_recv(s2, 10)
    if d[:1] == bytes([0x10]) and d[1:33] == bytes.fromhex(d1):
        got = d
        break
ok = got is not None and got[45:45 + struct.unpack("<I", got[41:45])[0]] == blob2
check("live message from phone 1 reaches phone 2", ok, got)
st, c1 = req("GET", "/mobile/contacts", token=tok1)
check("phone 2 shows online", any(x["peer_id"] == d2 and x["online"] for x in c1["contacts"]))
st, r = req("GET", "/mobile/inbox", token=tok2)
ids = [x["id"] for x in r["messages"] if x["from_peer_id"] == d1]
req("POST", "/mobile/inbox/ack", {"ids": ids}, token=tok2)
st, r = req("GET", "/mobile/inbox", token=tok2)
check("after ack phone 2's inbox is empty", not [x for x in r["messages"] if x["from_peer_id"] == d1], r)
s2.close(); s1.close()

# ---- internet through the PC
def connect_via(token, target, send=b"", proxy_auth=True):
    c = ctx().wrap_socket(socket.create_connection(("127.0.0.1", PORT), timeout=15))
    hdr = f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n" + (f"Proxy-Authorization: Bearer {token}\r\n" if proxy_auth else "") + "\r\n"
    c.sendall(hdr.encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        x = c.recv(1024)
        if not x:
            break
        buf += x
    status = buf.split(b"\r\n")[0]
    echo = b""
    if b" 200 " in status and send:
        c.sendall(send)
        c.settimeout(10)
        while len(echo) < len(send):
            x = c.recv(4096)
            if not x:
                break
            echo += x
    c.close()
    return status, echo

st1, _e = connect_via(tok1, "example.org:80", proxy_auth=False)
check("CONNECT without a token is refused", b"407" in st1, st1)
st1, _e = connect_via("0" * 64, "example.org:80")
check("CONNECT with a wrong token is refused", b"407" in st1, st1)
st1, _e = connect_via(tok1, "127.0.0.1:22")
check("CONNECT to the PC itself is refused", b"403" in st1, st1)
st1, _e = connect_via(tok1, "192.168.1.1:80")
check("CONNECT to the home network is refused", b"403" in st1, st1)
# the owner's upstream proxy: a fake SOCKS5 server that connects to a local echo
import threading
echo_srv = socket.socket(); echo_srv.bind(("127.0.0.1", 0)); echo_srv.listen(5)
def echo_loop():
    while True:
        try:
            cs, _ = echo_srv.accept()
        except Exception:
            return
        def one(cs=cs):
            while True:
                d = cs.recv(4096)
                if not d:
                    break
                cs.sendall(d)
            cs.close()
        threading.Thread(target=one, daemon=True).start()
threading.Thread(target=echo_loop, daemon=True).start()
seen_targets = []
px = socket.socket(); px.bind(("127.0.0.1", 0)); px.listen(5)
def rd(c, n):
    b = b""
    while len(b) < n:
        x = c.recv(n - len(b))
        if not x:
            raise EOFError
        b += x
    return b
def px_loop():
    while True:
        try:
            cs, _ = px.accept()
        except Exception:
            return
        def one(cs=cs):
            try:
                rd(cs, 1); n = rd(cs, 1)[0]; rd(cs, n); cs.sendall(b"\x05\x00")
                rd(cs, 3); t = rd(cs, 1)[0]
                host = rd(cs, rd(cs, 1)[0]).decode() if t == 3 else ""
                rd(cs, 2)
                seen_targets.append(host)
                up = socket.create_connection(echo_srv.getsockname())
                cs.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
                def pipe(x, y):
                    try:
                        while True:
                            d = x.recv(4096)
                            if not d:
                                break
                            y.sendall(d)
                    except Exception:
                        pass
                threading.Thread(target=pipe, args=(up, cs), daemon=True).start()
                pipe(cs, up)
            except Exception:
                pass
        threading.Thread(target=one, daemon=True).start()
threading.Thread(target=px_loop, daemon=True).start()
st, _r = a.api("POST", "/api/upstream-proxy", {"enabled": True, "kind": "socks5", "host": "127.0.0.1", "port": px.getsockname()[1], "auth": "none"})
check("owner's upstream proxy accepted", st == 200, (st, _r))
st1, echo = connect_via(tok2, "youtube.com:443", b"hello through the pc")
check("phone 2 reaches the internet through the PC's proxy (echo)", b" 200 " in st1 and echo == b"hello through the pc", (st1, echo))
check("the proxy got the NAME, not a local lookup", "youtube.com" in seen_targets, seen_targets)
# the proxy still works next to the API, and the decoy still answers a stranger
h2 = http.client.HTTPSConnection("127.0.0.1", PORT, context=ctx(), timeout=10)
h2.request("GET", "/")
check("a stranger still gets the decoy page", h2.getresponse().status == 200)
for x in (a, b):
    x.kill()
print("FAILED:" if fails else "ALL OK", fails or "")
sys.exit(1 if fails else 0)
