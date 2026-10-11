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

def rawreq(method, path, data=None, token=None):
    h = http.client.HTTPSConnection("127.0.0.1", PORT, context=ctx(), timeout=15)
    h.request(method, path, data, {"Authorization": "Bearer " + token, "Content-Type": "application/octet-stream"})
    r = h.getresponse()
    return r.status, r.read()

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

st, r = a.api("POST", "/api/mobile/groups", {"name": "x", "kind": "invite", "owner_device": devs["owner"]})
check("the node page can no longer create groups", st in (404, 405), st)
st, r = req("POST", "/mobile/groups", {"name": "", "topic": "тест", "kind": "invite"}, token=t_own)
check("a group needs a name", st == 400, (st, r))
st, r = req("POST", "/mobile/groups", {"name": "Семья", "topic": "домашнее", "about": "для своих", "kind": "invite"}, token=t_own)
check("a phone creates a group with topic and description", st == 200 and len(r.get("id", "")) == 32, (st, r))
gid = r["id"]
st, r = req("GET", "/mobile/groups/open", token=t_bob)
check("an invite-only group is listed for other phones with topic and description", [x for x in r["groups"] if x["id"] == gid][0]["about"] == "для своих", r)
st, r = req("POST", "/mobile/groups", {"name": "Без темы", "kind": "invite"}, token=t_own)
check("a group without a topic is refused", st == 400, (st, r))
st, r = req("POST", "/mobile/groups", {"name": "Тип", "topic": "x", "kind": "closed"}, token=t_own)
check("the removed 'closed' type is refused on create", st == 400, (st, r))
st, r = req("GET", "/mobile/groups", token=t_own)
check("the list carries topic and description", [x for x in r["groups"] if x["id"] == gid][0]["topic"] == "домашнее" and [x for x in r["groups"] if x["id"] == gid][0]["about"] == "для своих", r)
st, r = req("POST", f"/mobile/groups/{gid}/join", {}, token=t_bob)
check("an invite group cannot be joined without an invitation", st == 403, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/invite", {"uses": "invalid", "ttl_hours": []}, token=t_own)
check("an invite request with wrong types is refused, not filled with defaults", st == 400, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/invite", {"uses": 1}, token=t_own)
check("the owner issues a code for the group", st == 200 and len(r.get("code", "")) == 14, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/join", {"invite_code": r["code"]}, token=t_bob)
check("bob joins the group with the code", st == 200 and r.get("alias"), (st, r))

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
st, r = req("POST", "/mobile/groups", {"name": "Клуб", "topic": "клуб", "kind": "invite"}, token=t_own)
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
st, r = req("POST", "/mobile/groups", {"name": "Двор", "topic": "двор", "kind": "open"}, token=t_own)
gid3 = r["id"]
st, r = req("GET", "/mobile/groups/open", token=t_eve)
check("eve sees the open group in the node's list (with the invite-only ones)", st == 200 and [x for x in r["groups"] if x["id"] == gid3] and [x for x in r["groups"] if x["id"] == gid3][0]["kind"] == "open", r)
st, r = req("POST", f"/mobile/groups/{gid3}/join", {}, token=t_eve)
eve_alias = r.get("alias")
check("eve joins the open group", st == 200 and eve_alias, (st, r))
st, _ = a.api("POST", f"/api/mobile/groups/{gid3}/mod", {"action": "ban", "alias": eve_alias})
check("the node page can no longer moderate", st in (404, 405), st)
st, _ = a.api("POST", f"/api/mobile/groups/{gid3}/members", {"device": devs["bob"]})
check("the node page can no longer add members", st in (404, 405), st)
st, _ = a.api("POST", f"/api/mobile/groups/{gid3}/owner", {"device": devs["bob"]})
check("the node page can no longer change the owner", st in (404, 405), st)
st, _ = a.api("POST", f"/api/mobile/groups/{gid3}/invite", {})
check("the node page can no longer issue invites", st in (404, 405), st)
st, _ = req("POST", f"/mobile/groups/{gid3}/mod", {"action": "ban", "alias": eve_alias}, token=t_own)
check("the page can ban", st == 200, st)
st, r = req("POST", f"/mobile/groups/{gid3}/join", {}, token=t_eve)
check("a banned phone cannot come back", st == 403, st)
st, r = req("GET", "/mobile/groups", token=t_own)
g3 = [x for x in r["groups"] if x["id"] == gid3]
check("the owner is told to change the key after a ban", g3 and g3[0]["rekey_needed"], r)

# a phone creates its own group and owns it
st, r = req("POST", "/mobile/groups", {"name": "Свои", "topic": "x", "kind": "closed"}, token=t_bob)
check("a phone cannot create a closed group", st == 400, (st, r))
st, r = req("POST", "/mobile/groups", {"name": "", "kind": "invite"}, token=t_bob)
check("a phone group needs a name", st == 400, (st, r))
st, r = req("POST", "/mobile/groups", {"name": "Бобова", "topic": "мои", "kind": "invite"}, token=t_bob)
gid4 = r.get("id")
check("a phone creates a group", st == 200 and gid4 and len(gid4) == 32, (st, r))
st, r = req("GET", "/mobile/groups", token=t_bob)
mine = [x for x in r["groups"] if x["id"] == gid4]
check("the creating phone is its owner", mine and mine[0]["role"] == "owner", r)
st, r = req("POST", f"/mobile/groups/{gid4}/invite", {"uses": 1}, token=t_bob)
check("it can issue invites itself", st == 200 and len(r.get("code", "")) == 14, (st, r))
code4 = r.get("code")
st, r = req("POST", f"/mobile/groups/{gid4}/join", {"invite_code": code4}, token=t_own)
check("another phone joins by that invite", st == 200, (st, r))
st, r = req("GET", f"/mobile/groups/{gid4}/modlog", token=t_bob)
check("the log names the phone, not the node page", st == 200 and r["modlog"][0]["by"] != "компьютер" and r["modlog"][0]["action"] == "create", r)
# asking for an invitation: eve asks, only the owner/moderators see it and answer
st, r = req("GET", "/mobile/groups/open", token=t_eve)
g4 = [x for x in r["groups"] if x["id"] == gid4]
check("an invite-only group is listed for others with its kind", g4 and g4[0]["kind"] == "invite" and g4[0]["requested"] is False, r)
st, r = req("POST", f"/mobile/groups/{gid4}/request", {"name": "Ева"}, token=t_eve)
check("eve asks for an invitation", st == 200, (st, r))
st, r = req("POST", f"/mobile/groups/{gid4}/request", {"name": "Ева"}, token=t_eve)
check("a second request is refused", st == 409, (st, r))
st, r = req("GET", "/mobile/groups/open", token=t_eve)
check("the list says the request was sent", [x for x in r["groups"] if x["id"] == gid4][0]["requested"] is True, r)
st, r = req("GET", f"/mobile/groups/{gid4}/requests", token=t_eve)
check("the requester cannot read requests", st == 403, st)
st, r = req("GET", f"/mobile/groups/{gid4}/requests", token=t_bob)
rq = r.get("requests", []) if st == 200 else []
check("the owner sees the request with the name", len(rq) == 1 and rq[0]["name"] == "Ева" and len(rq[0]["peer_id"]) == 64, (st, r))
st, r = req("GET", "/mobile/groups", token=t_bob)
check("the owner's list counts pending requests", [x for x in r["groups"] if x["id"] == gid4][0]["requests"] == 1, r)
st, _ = req("DELETE", f"/mobile/groups/{gid4}/requests/{rq[0]['id']}", token=t_eve)
check("the requester cannot clear the request", st == 403, st)
st, _ = req("DELETE", f"/mobile/groups/{gid4}/requests/{rq[0]['id']}", token=t_bob)
check("the owner clears it after answering", st == 200, st)
st, r = req("GET", f"/mobile/groups/{gid4}/requests", token=t_bob)
check("no requests left, no device id kept", st == 200 and r["requests"] == [], r)
st, r = req("POST", f"/mobile/groups/{gid3}/request", {"name": "x"}, token=t_eve)
check("a request to an open or closed group is refused", st in (400, 409), (st, r))

for i in range(9):
    st, r = req("POST", "/mobile/groups", {"name": f"g{i}", "topic": "t", "kind": "open"}, token=t_bob)
st, r = req("POST", "/mobile/groups", {"name": "лишняя", "topic": "t", "kind": "open"}, token=t_bob)
check("a phone cannot lead more than ten groups", st == 429, (st, r))

# files of a group: only members, only ciphertext chunks, expiry/limits, uploader or moderator deletes
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 1000, "total_chunks": 1}, token=t_eve)
check("an outsider cannot start a group file", st == 403, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 201 << 20, "total_chunks": 1000}, token=t_bob)
check("a group file over 200 MB is refused", st == 413, (st, r))
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 600 * 1024, "total_chunks": 3}, token=t_bob)
fid = r.get("file_id")
check("a member starts a group file", st == 200 and fid and len(fid) == 32, (st, r))
st, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fid}/chunk/0", b"A" * 1000, token=t_own)
check("only the uploader puts chunks", st == 403, st)
for i in (0, 1):
    st, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fid}/chunk/{i}", bytes([65 + i]) * 5000, token=t_bob)
    check(f"chunk {i} stored", st == 200, st)
st, r = req("POST", f"/mobile/groups/{gid}/files/{fid}/done", {}, token=t_bob)
check("done is refused while a chunk is missing", st == 409 and r.get("missing") == [2], (st, r))
st, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fid}/chunk/2", b"C" * 5000, token=t_bob)
st, r = req("POST", f"/mobile/groups/{gid}/files/{fid}/done", {}, token=t_bob)
check("done after all chunks", st == 200, (st, r))
st, b = rawreq("GET", f"/mobile/groups/{gid}/files/{fid}/chunk/1", token=t_own)
check("another member downloads a chunk byte-for-byte", st == 200 and b == b"B" * 5000, st)
st, _ = rawreq("GET", f"/mobile/groups/{gid}/files/{fid}/chunk/1", token=t_eve)
check("an outsider cannot download", st == 403, st)
st, _ = rawreq("DELETE", f"/mobile/groups/{gid}/files/{fid}", token=t_eve)
check("an outsider cannot delete", st == 403, st)
st, _ = rawreq("DELETE", f"/mobile/groups/{gid}/files/{fid}", token=t_own)
check("the owner can delete a member's file", st == 200, st)
st, _ = rawreq("GET", f"/mobile/groups/{gid}/files/{fid}/chunk/1", token=t_own)
check("a deleted file is gone", st == 404, st)
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 1000, "total_chunks": 1}, token=t_bob)
fid2 = r.get("file_id")
rawreq("PUT", f"/mobile/groups/{gid}/files/{fid2}/chunk/0", b"Z" * 1000, token=t_bob)
req("POST", f"/mobile/groups/{gid}/files/{fid2}/done", {}, token=t_bob)

# real bytes are bounded by the declared size (quota cannot be bypassed with a tiny declaration)
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 100, "total_chunks": 1}, token=t_bob)
fq = r.get("file_id")
st, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fq}/chunk/0", b"Q" * 5000, token=t_bob)
check("a chunk larger than the declared file is refused", st == 400, st)
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 1000, "total_chunks": 2}, token=t_bob)
fq2 = r.get("file_id")
st1, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fq2}/chunk/0", b"A" * 1000, token=t_bob)
st2, _ = rawreq("PUT", f"/mobile/groups/{gid}/files/{fq2}/chunk/1", b"B" * 1000, token=t_bob)
check("an unfinished upload cannot hold more than the declared size", st1 == 200 and st2 == 400, (st1, st2))
rawreq("DELETE", f"/mobile/groups/{gid}/files/{fq2}", token=t_bob)
st, r = req("POST", f"/mobile/groups/{gid}/files", {"file_size": 3, "total_chunks": 4}, token=t_bob)
check("more chunks than bytes is refused", st == 400, (st, r))
rawreq("DELETE", f"/mobile/groups/{gid}/files/{fq}", token=t_bob)

# deleting: only on the page, and members are told
st, r = req("DELETE", f"/mobile/groups/{gid}", token=t_own)
check("a phone cannot delete a group", st in (404, 405), st)
st, _ = a.api("DELETE", f"/api/mobile/groups/{gid}")
check("the old password-less delete route is gone", st in (404, 405), st)
st, _ = a.api("POST", f"/api/mobile/groups/{gid}/delete", {})
check("deleting without a password is refused", st in (400, 422), st)
st, _ = a.api("POST", f"/api/mobile/groups/{gid}/delete", {"password": "wrong-password"})
check("deleting with a wrong password is refused", st == 401, st)
st, r = req("GET", f"/mobile/groups/{gid}/log?after=0", token=t_own)
check("the group is still there after refused deletes", st == 200, st)
time.sleep(4)   # задержка после неверного пароля — как при входе
for _ in range(10):
    st, _ = a.api("POST", f"/api/mobile/groups/{gid}/delete", {"password": C.LOGIN})
    if st != 429:
        break
    time.sleep(3)
check("the page deletes the group with the node password", st == 200, st)
h = hint(s_bob, gid, kind=4)
check("bob is told the group is gone", h is not None, h)
st, r = req("GET", f"/mobile/groups/{gid}/log?after=0", token=t_bob)
check("its journal is gone", st == 404, st)
check("its files are gone from the disk", not os.path.exists(os.path.join(a.dir, "home/.local/share/yandi/groups/files", gid)))
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
