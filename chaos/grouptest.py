#!/usr/bin/env python3
"""Groups end to end on one node: created on the node's page, used by phones over the TLS entry (journal, pull, hint frame,
keys, invites, moderation), deleted on the page. The script plays the owner's page and three phones."""
import sys, os, time, json, ssl, socket, struct, base64, http.client
sys.path.insert(0, os.path.dirname(__file__))
import chaos as C

PORT = 26950
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/grouptest"
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
    return s, buf.split(b"\r\n")[0]

def ws_recv(s, timeout=10):
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

def hint(s, gid, timeout=10, kind=None):
    """The next group hint frame for this group (of this kind, if given): (kind, seq), or None."""
    end = time.time() + timeout
    try:
        while time.time() < end:
            _, d = ws_recv(s, max(0.1, end - time.time()))
            if d[:1] == b"\x15" and d[1:17] == bytes.fromhex(gid) and kind in (None, d[17]):
                return d[17], struct.unpack("<Q", d[18:26])[0]
    except Exception:
        pass
    return None

def b64(b):
    return base64.b64encode(b).decode()

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
    return r["token"]

t_own, t_bob, t_eve = pair_phone("owner"), pair_phone("bob"), pair_phone("eve")
st, g = a.api("GET", "/api/mobile/groups")
devs = {d["name"]: d["id"] for d in g.get("devices", [])}
check("the page lists paired devices for groups", st == 200 and {"owner", "bob", "eve"} <= set(devs), (st, g))

st, r = req("POST", "/mobile/groups", {"name": "x"}, token=t_own)
check("a phone cannot create a group", st in (404, 405), st)
st, r = a.api("POST", "/api/mobile/groups", {"name": "", "kind": "closed", "owner_device": devs["owner"]})
check("a group needs a name", st == 400, (st, r))
st, r = a.api("POST", "/api/mobile/groups", {"name": "Семья", "topic": "домашнее", "kind": "closed", "owner_device": devs["owner"], "members": [devs["bob"]]})
check("the page creates a group", st == 200 and len(r.get("id", "")) == 32, (st, r))
gid = r["id"]

st, r = req("GET", "/mobile/groups", token=t_own)
mine = r["groups"][0] if st == 200 and r["groups"] else {}
check("owner phone sees the group as owner", mine.get("role") == "owner" and mine.get("name") == "Семья", r)
st, r = req("GET", "/mobile/groups", token=t_bob)
bob_alias = r["groups"][0]["my_alias"] if st == 200 and r["groups"] else None
check("bob is a member under a pseudonym", bob_alias and r["groups"][0]["role"] == "member", r)
st, r = req("GET", "/mobile/groups", token=t_eve)
check("eve is not in the group", st == 200 and r["groups"] == [], r)
st, r = req("POST", f"/mobile/groups/{gid}/join", {}, token=t_eve)
check("nobody joins a closed group from a phone", st == 403, (st, r))

# keys: bob publishes his group key, owner seals the group key for him
bob_pub = b64(os.urandom(32))
st, _ = req("POST", f"/mobile/groups/{gid}/pubkey", {"member_pub": bob_pub}, token=t_bob)
check("member publishes a group key", st == 200, st)
st, r = req("GET", f"/mobile/groups/{gid}/members", token=t_own)
bm = [m for m in r.get("members", []) if m["alias"] == bob_alias]
check("owner sees bob's key and that he has no sealed key yet", bm and bm[0]["member_pub"] == bob_pub and not bm[0]["has_key"], r)
st, r = req("POST", f"/mobile/groups/{gid}/keys", {"epoch": 1, "boxes": {bob_alias: b64(b"sealed-for-bob")}}, token=t_bob)
check("only the owner hands out keys", st == 403, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/keys", {"epoch": 1, "boxes": {bob_alias: b64(b"sealed-for-bob")}}, token=t_own)
check("owner stores a sealed key", st == 200 and r.get("stored") == 1, (st, r))
st, r = req("GET", f"/mobile/groups/{gid}/keys", token=t_bob)
check("bob gets only his sealed key", st == 200 and r["epoch"] == 1 and [x["box"] for x in r["boxes"]] == [b64(b"sealed-for-bob")], r)

# journal + hint
s_bob, line = ws_open(t_bob)
check("bob's socket opens", b"101" in line, line)
time.sleep(0.5)
st, r = req("POST", f"/mobile/groups/{gid}/log", {"epoch": 1, "payload_b64": b64(b"\xe2ciphertext-1")}, token=t_own)
check("owner appends an entry", st == 200 and r.get("seq") == 1, (st, r))
h = hint(s_bob, gid)
check("bob gets a hint without content", h == (1, 1), h)
st, r = req("POST", f"/mobile/groups/{gid}/log", {"epoch": 1, "payload_b64": b64(b"\xe2ciphertext-2")}, token=t_bob)
check("bob appends", st == 200 and r.get("seq") == 2, (st, r))
st, r = req("GET", f"/mobile/groups/{gid}/log?after=1", token=t_bob)
e = r.get("entries", []) if st == 200 else []
check("pull after 1 gives entry 2 with bob's pseudonym, bytes untouched", len(e) == 1 and e[0]["seq"] == 2 and e[0]["from"] == bob_alias and base64.b64decode(e[0]["payload_b64"]) == b"\xe2ciphertext-2", r)
st, r = req("GET", f"/mobile/groups/{gid}/log?after=0", token=t_eve)
check("an outsider cannot pull", st == 403, st)
st, r = req("POST", f"/mobile/groups/{gid}/log", {"epoch": 1, "payload_b64": b64(b"x")}, token=t_eve)
check("an outsider cannot post", st == 403, st)

# moderation from the owner's phone
st, r = req("POST", f"/mobile/groups/{gid}/mod", {"action": "mute", "alias": bob_alias}, token=t_bob)
check("a plain member cannot moderate", st == 403, st)
st, _ = req("POST", f"/mobile/groups/{gid}/mod", {"action": "mute", "alias": bob_alias, "reason": "тест"}, token=t_own)
st, r = req("POST", f"/mobile/groups/{gid}/log", {"epoch": 1, "payload_b64": b64(b"x")}, token=t_bob)
check("muted bob cannot post", st == 403, st)
req("POST", f"/mobile/groups/{gid}/mod", {"action": "unmute", "alias": bob_alias}, token=t_own)
st, _ = req("POST", f"/mobile/groups/{gid}/mod", {"action": "delete_entry", "seq": 2}, token=t_own)
st, r = req("GET", f"/mobile/groups/{gid}/log?after=1", token=t_bob)
check("a deleted entry stays numbered but empty", st == 200 and r["entries"][0]["deleted"] is True and r["entries"][0]["payload_b64"] == "", r)
st, r = req("GET", f"/mobile/groups/{gid}/modlog", token=t_bob)
acts = [x["action"] for x in r.get("modlog", [])]
check("the moderation log is kept on the node", acts[-3:] == ["mute", "unmute", "delete_entry"], acts)

# invite group: invite code from the owner phone, eve joins
st, r = a.api("POST", "/api/mobile/groups", {"name": "Клуб", "kind": "invite", "owner_device": devs["owner"]})
gid2 = r["id"]
st, r = req("POST", f"/mobile/groups/{gid2}/invite", {"uses": 1}, token=t_own)
code = r.get("code") if st == 200 else None
check("owner phone issues an invite code", code and len(code) == 14, (st, r))
st, r = req("POST", f"/mobile/groups/{gid2}/join", {"invite_code": "AAAA-BBBB-CCCC"}, token=t_eve)
check("a wrong code is refused", st == 403, st)
st, r = req("POST", f"/mobile/groups/{gid2}/join", {"invite_code": code}, token=t_eve)
check("eve joins with the code", st == 200 and r.get("alias"), (st, r))
st, r = req("POST", f"/mobile/groups/{gid2}/join", {"invite_code": code}, token=t_bob)
check("the code works once", st == 403, st)

# open group: seen and joined by any phone of the node
st, r = a.api("POST", "/api/mobile/groups", {"name": "Двор", "kind": "open", "owner_device": devs["owner"]})
gid3 = r["id"]
st, r = req("GET", "/mobile/groups/open", token=t_eve)
check("eve sees the open group", st == 200 and [x["id"] for x in r["groups"]] == [gid3], r)
st, r = req("POST", f"/mobile/groups/{gid3}/join", {}, token=t_eve)
eve_alias = r.get("alias")
check("eve joins the open group", st == 200 and eve_alias, (st, r))
st, _ = a.api("POST", f"/api/mobile/groups/{gid3}/mod", {"action": "ban", "alias": eve_alias})
check("the page can ban", st == 200, st)
st, r = req("POST", f"/mobile/groups/{gid3}/join", {}, token=t_eve)
check("a banned phone cannot come back", st == 403, st)
st, r = req("GET", "/mobile/groups", token=t_own)
g3 = [x for x in r["groups"] if x["id"] == gid3]
check("the owner is told to change the key after a ban", g3 and g3[0]["rekey_needed"], r)

# deleting: only on the page, and members are told
st, r = req("DELETE", f"/mobile/groups/{gid}", token=t_own)
check("a phone cannot delete a group", st in (404, 405), st)
st, _ = a.api("DELETE", f"/api/mobile/groups/{gid}")
check("the page deletes the group", st == 200, st)
h = hint(s_bob, gid, kind=4)
check("bob is told the group is gone", h is not None, h)
st, r = req("GET", f"/mobile/groups/{gid}/log?after=0", token=t_bob)
check("its journal is gone", st == 404, st)
s_bob.close()

# a device forgotten by the node leaves its groups
st, g = a.api("GET", "/api/mobile/groups")
c = [x for x in g["groups"] if x["id"] == gid2][0]
n_before = len(c["members"])
a.api("DELETE", f"/api/mobile/devices/{devs['eve']}")
st, g = a.api("GET", "/api/mobile/groups")
c = [x for x in g["groups"] if x["id"] == gid2][0]
check("forgetting a device drops it from groups", len(c["members"]) == n_before - 1, c)
raw = open(os.path.join(a.dir, "home/.local/share/yandi/groups/index.json")).read()
st, i = req("GET", "/mobile/info", token=t_bob)
check("device ids are not stored in the groups file", i["device_id"] not in raw)

a.kill()
print("FAILED:" if fails else "ALL OK", fails or "")
sys.exit(1 if fails else 0)
