#!/usr/bin/env python3
"""The node as a group participant (PC): real group crypto from a Python phone, checks formats, keys, feed both ways and the closed vault."""
import sys, os, time, json, ssl, socket, struct, base64, http.client
sys.path.insert(0, os.path.dirname(__file__))
import chaos as C

PORT = 26951
out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/pcgroups"
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


from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

def raw_pub(sk):
    return sk.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)

def new_keys():
    sk = X25519PrivateKey.generate()
    seed = sk.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    return seed, raw_pub(sk)

def box_key(my_seed, their_pub, gid, epoch):
    shared = X25519PrivateKey.from_private_bytes(my_seed).exchange(X25519PublicKey.from_public_bytes(their_pub))
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=b"yandi-group-box:v1", info=f"{gid}|{epoch}".encode()).derive(shared)

def seal(key, aad, plain):
    n = os.urandom(12)
    return n + AESGCM(key).encrypt(n, plain, aad)

def opn(key, aad, wire):
    return AESGCM(key).decrypt(wire[:12], wire[12:], aad)

def seal_key(gk, gid, epoch, owner_seed, member_pub):
    return seal(box_key(owner_seed, member_pub, gid, epoch), f"yandi-group-key:v1|{gid}|{epoch}".encode(), gk)

def open_key(wire, gid, epoch, my_seed, owner_pub):
    return opn(box_key(my_seed, owner_pub, gid, epoch), f"yandi-group-key:v1|{gid}|{epoch}".encode(), wire)

def enc_entry(gk, gid, epoch, frm, kind, body):
    return bytes([1, kind]) + seal(gk, f"yandi-group-entry:v1|{gid}|{epoch}|{frm}|{kind}".encode(), json.dumps(body, ensure_ascii=False).encode())

def dec_entry(gk, payload, gid, epoch, frm):
    kind = payload[1]
    return kind, json.loads(opn(gk, f"yandi-group-entry:v1|{gid}|{epoch}|{frm}|{kind}".encode(), payload[2:]))

a = C.Node(1, out, binary, anchor=True)
a.start()
assert a.login(120) and a.ready(120), a.k

def pair_phone(name):
    st, q = a.api("GET", "/api/mobile/pairing?host=127.0.0.1")
    code = pair_text(q["qr_text"])["pairing_code"]
    st, r = req("POST", "/mobile/pair", {"pairing_code": code, "device_name": name})
    return r["token"]

# перед сопряжением: включаем учёт контактов (устройства, уже бывшие на узле, остаются контактами; новые — «не в контактах»)
a.api("GET", "/api/mobile/other-devices")
t_ph = pair_phone("phone")

class Phone:
    """A phone's group client in miniature: the same crypto as the app."""
    def __init__(self, token):
        self.t = token
        self.keys = {}
        self.seed, self.pub = new_keys()
    def call(self, m, path, body=None):
        return req(m, path, body, token=self.t)
    def publish(self, gid):
        return self.call("POST", f"/mobile/groups/{gid}/pubkey", {"member_pub": b64(self.pub)})
    def provision(self, gid):
        """as the owner: first epoch, then sealed keys for everyone who has a public key but no sealed key"""
        st, groups = self.call("GET", "/mobile/groups")
        g = [x for x in groups["groups"] if x["id"] == gid][0]
        st, m = self.call("GET", f"/mobile/groups/{gid}/members")
        others = [x for x in m["members"] if x["role"] != "owner" and x["member_pub"]]
        epoch = g["epoch"]
        if epoch == 0 or g["rekey_needed"]:
            epoch += 1
            self.keys[epoch] = os.urandom(32)
            targets = others
        else:
            targets = [x for x in others if not x["has_key"]]
            if not targets:
                return epoch
        boxes = {x["alias"]: b64(seal_key(self.keys[epoch], gid, epoch, self.seed, base64.b64decode(x["member_pub"]))) for x in targets}
        st, r = self.call("POST", f"/mobile/groups/{gid}/keys", {"epoch": epoch, "boxes": boxes})
        assert st == 200, (st, r)
        return epoch
    def collect(self, gid, owner_pub=None):
        """owner_pub: the pinned key of the previous owner (used when this phone is the owner now and misses old generations)"""
        st, k = self.call("GET", f"/mobile/groups/{gid}/keys")
        pub = owner_pub or k.get("owner_pub")
        if st != 200 or not pub:
            return
        for b in k["boxes"]:
            if b["epoch"] not in self.keys:
                try:
                    self.keys[b["epoch"]] = open_key(base64.b64decode(b["box"]), gid, b["epoch"], self.seed, base64.b64decode(pub))
                except Exception:
                    pass
    def read(self, gid):
        st, p = self.call("GET", f"/mobile/groups/{gid}/log?after=0")
        out = []
        for e in p["entries"]:
            if e["deleted"] or e["epoch"] not in self.keys:
                continue
            kind, body = dec_entry(self.keys[e["epoch"]], base64.b64decode(e["payload_b64"]), gid, e["epoch"], e["from"])
            out.append((e["from"], kind, body))
        return out
    def write(self, gid, alias, kind, body):
        epoch = max(self.keys)
        return self.call("POST", f"/mobile/groups/{gid}/log", {"epoch": epoch, "payload_b64": b64(enc_entry(self.keys[epoch], gid, epoch, alias, kind, body))})

def wait(cond, secs=40, step=1.0):
    t0 = time.time()
    while time.time() - t0 < secs:
        v = cond()
        if v:
            return v
        time.sleep(step)
    return None

ph = Phone(t_ph)

# -1) one page for groups; the old inter-node groups and the extra pages are gone
st, body = a.api("GET", "/groups")
text = body.decode() if isinstance(body, (bytes, bytearray)) else str(body)
check("/groups serves the unified page (my groups + node management)", st == 200 and "tabMineBtn" in text and "tabAdminBtn" in text and "mgFeed" in text and "delDlg" in text, st)
for gone in ["/my-groups", "/phone-groups", "/group-chat", "/api/groups/sync", "/api/groups/dht/status", "/api/group-chat/history/aa"]:
    st, _ = a.api("GET", gone)
    check(f"{gone} no longer exists", st in (404, 405), st)
st, r = a.api("GET", "/api/groups")
check("GET /api/groups is the participant list with the node catalog", st == 200 and "groups" in r and "catalog" in r, (st, r))

# 0) devices that are not contacts: new -> add / hide / block
st, r = a.api("GET", "/api/mobile/other-devices")
dv = [x for x in r["devices"] if x["name"].startswith("phone")]
check("a freshly paired phone is listed as 'not in contacts'", st == 200 and dv and dv[0]["status"] == "new", r)
peer_id = dv[0]["peer_id"]
def in_contacts():
    st, c = a.api("GET", "/api/contacts")
    return any(x.get("short_id") == peer_id for x in c.get("contacts", []))
check("it is not in the chat contacts yet", not in_contacts())
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "hide"})
st, r = a.api("GET", "/api/mobile/other-devices")
check("hide moves it to hidden", [x for x in r["devices"] if x["peer_id"] == peer_id][0]["status"] == "hidden", r)
a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "unhide"})
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "block"})
st, r2 = a.api("GET", "/api/mobile/other-devices")
check("block marks it blocked and it stays out of contacts", [x for x in r2["devices"] if x["peer_id"] == peer_id][0]["status"] == "blocked" and not in_contacts(), r2)
st, r = a.api("POST", f"/api/chat/send/{peer_id}", {"text": "to a blocked device"})
check("the PC does not send to a blocked device", r.get("status") == "error" and "заблокировано" in r.get("message", ""), r)
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "unblock"})
st, r = a.api("POST", f"/api/chat/send/{peer_id}", {"text": "after unblock"})
check("after unblocking the block no longer stops sending", "заблокировано" not in str(r.get("message", "")), r)
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "add"})
st, r2 = a.api("GET", "/api/mobile/other-devices")
check("add puts it into the contacts and out of the 'other' list", in_contacts() and not [x for x in r2["devices"] if x["peer_id"] == peer_id], r2)
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": "zz", "action": "add"})
check("a bad device number is refused", st == 400, (st, r))
st, r = a.api("POST", "/api/mobile/phone-action", {"peer": peer_id, "action": "explode"})
check("an unknown action is refused", st == 400, (st, r))

# 1) the PC creates a group and owns it; the phone sees it and joins
st, r = a.api("POST", "/api/groups/create", {"name": "Домашняя", "topic": "с ПК", "kind": "closed"})
check("the PC cannot create the removed closed type", st == 400, (st, r))
st, r = a.api("POST", "/api/groups/create", {"name": "Без темы", "kind": "open"})
check("the PC cannot create a group without a topic", st == 400, (st, r))
st, r = a.api("POST", "/api/groups/create", {"name": "Домашняя", "topic": "с ПК", "about": "общая лента", "kind": "open"})
gid = r.get("id")
check("the PC creates an open group", st == 200 and gid and len(gid) == 32, (st, r))
st, r = a.api("GET", "/api/groups")
mine = [x for x in r["groups"] if x["id"] == gid]
check("topic and description are in the list", mine and mine[0]["topic"] == "с ПК" and mine[0]["about"] == "общая лента", r)
check("the PC is the owner and has the first key", mine and mine[0]["role"] == "owner" and mine[0]["has_key"] is True and mine[0]["epoch"] == 1, r)
st, r = ph.call("GET", "/mobile/groups/open")
check("the phone sees the PC's group in the node's list", [x["name"] for x in r["groups"]] == ["Домашняя"], r)
st, r = ph.call("POST", f"/mobile/groups/{gid}/join", {"member_pub": b64(ph.pub)})
alias = r.get("alias")
check("the phone joins the open group", st == 200 and alias, (st, r))
got = wait(lambda: (ph.collect(gid), ph.keys)[1])
check("the PC hands the phone a sealed key (phone opens it with its own crypto)", bool(got) and len(ph.keys[1]) == 32, ph.keys)

# 2) messages both ways
st, r = a.api("POST", f"/api/groups/{gid}/send", {"text": "привет с компьютера 😀"})
check("the PC posts to the group", st == 200 and r.get("seq"), (st, r))
rd = ph.read(gid)
check("the phone reads the PC's message (shared format)", any(b.get("t") == "привет с компьютера 😀" for _, _, b in rd), rd)
st, r = ph.write(gid, alias, 1, {"t": "ответ с телефона"})
check("the phone posts", st == 200, (st, r))
def pc_sees():
    st, d = a.api("GET", f"/api/groups/{gid}/messages?after=0")
    return d if any(m["body"].get("t") == "ответ с телефона" for m in d.get("messages", [])) else None
d = wait(pc_sees)
check("the PC reads the phone's message", d is not None, d)
check("the author is shown by pseudonym", d and [m for m in d["messages"] if m["mine"] is False][0]["from"] == alias, d)

# 2b) files: phone -> PC and PC -> phone in the app's chunk format; tampering, outsiders
import hashlib
CH = 256 * 1024
def phone_upload(phone, gid_, alias_, name, data, kind):
    fkey = os.urandom(32)
    total = (len(data) + CH - 1) // CH
    st, r = phone.call("POST", f"/mobile/groups/{gid_}/files", {"file_size": len(data), "total_chunks": total})
    fid = r["file_id"]
    for i in range(total):
        part = data[i * CH:(i + 1) * CH]
        wire = seal(fkey, f"yandi-file:v1|{fid}|{i}|{total}".encode(), part)
        rawreq("PUT", f"/mobile/groups/{gid_}/files/{fid}/chunk/{i}", wire, token=phone.t)
    phone.call("POST", f"/mobile/groups/{gid_}/files/{fid}/done", {})
    body = {"f": fid, "n": name, "z": len(data), "c": total, "k": b64(fkey), "h": hashlib.sha256(data).hexdigest()}
    st, r = phone.write(gid_, alias_, kind, body)
    return fid, r.get("seq")
blob = os.urandom(600 * 1024 + 77)
fid_ph, seq_ph = phone_upload(ph, gid, alias, "снимок.png", blob, 2)
def pc_has(seq_):
    st, d = a.api("GET", f"/api/groups/{gid}/messages?after=0")
    return d if any(m["seq"] == seq_ for m in d.get("messages", [])) else None
check("the PC sees the phone's file record", wait(lambda: pc_has(seq_ph)) is not None)
st, body, hd = a._http("GET", f"/api/groups/{gid}/file/{seq_ph}", None, a.cookie)
check("the PC downloads, decrypts and verifies the phone's file byte for byte", st == 200 and body == blob, (st, len(body) if isinstance(body, (bytes, bytearray)) else body))
check("a picture is shown in place, with nosniff and a sandbox", hd.get("Content-Type") == "image/png" and hd.get("X-Content-Type-Options") == "nosniff" and "sandbox" in (hd.get("Content-Security-Policy") or ""), dict(hd))
# upload from the PC (raw body)
import urllib.parse, http.client as HC
def pc_upload(name, data):
    h = HC.HTTPConnection("127.0.0.1", a.web, timeout=60)
    h.request("POST", f"/api/groups/{gid}/upload?name=" + urllib.parse.quote(name), data, {"Cookie": a.cookie, "Content-Type": "application/octet-stream"})
    r = h.getresponse(); raw = r.read()
    try:
        return r.status, json.loads(raw)
    except Exception:
        return r.status, raw
pc_blob = os.urandom(300 * 1024) + "<script>alert(1)</script>".encode()
st, r = pc_upload("отчёт.html", pc_blob)
check("the PC attaches a file to the group", st == 200 and r.get("seq"), (st, r))
pc_seq = r.get("seq")
st, p_ = ph.call("GET", f"/mobile/groups/{gid}/log?after={pc_seq - 1}")
e = p_["entries"][0]
kind, fb = dec_entry(ph.keys[e["epoch"]], base64.b64decode(e["payload_b64"]), gid, e["epoch"], e["from"])
got = b""
for i in range(fb["c"]):
    stc, wire = rawreq("GET", f"/mobile/groups/{gid}/files/{fb['f']}/chunk/{i}", None, token=ph.t)
    got += opn(base64.b64decode(fb["k"]), f"yandi-file:v1|{fb['f']}|{i}|{fb['c']}".encode(), wire)
check("the phone opens the PC's file with its own crypto (same format)", kind == 3 and got == pc_blob and hashlib.sha256(got).hexdigest() == fb["h"] and fb["n"] == "отчёт.html", (kind, len(got)))
st, body, hd = a._http("GET", f"/api/groups/{gid}/file/{pc_seq}", None, a.cookie)
check("an HTML attachment is never served as a page (download only)", hd.get("Content-Type") == "application/octet-stream" and (hd.get("Content-Disposition") or "").startswith("attachment"), dict(hd))
# tampering with a stored chunk on the node's disk
cpath = os.path.join(a.dir, f"home/.local/share/yandi/groups/files/{gid}/{fid_ph}/c000001")
raw = bytearray(open(cpath, "rb").read()); raw[40] ^= 1; open(cpath, "wb").write(bytes(raw))
st, body, hd = a._http("GET", f"/api/groups/{gid}/file/{seq_ph}", None, a.cookie)
check("a tampered chunk is detected and nothing is served", st == 400, st)
st, r = a.api("GET", f"/api/groups/{'0' * 32}/file/1")
check("a file of a group the PC is not in cannot be fetched", st == 400, (st, r))

# 3) name announce, rules
st, r = ph.write(gid, alias, 7, {"op": "name", "n": "Телефон-Боб"})
d = wait(lambda: (lambda x: x if any(m["author"] == "Телефон-Боб" for m in x["messages"]) else None)(a.api("GET", f"/api/groups/{gid}/messages?after=0")[1]))
check("the name the phone announced replaces the pseudonym on the PC", d is not None, d)
st, r = a.api("POST", f"/api/groups/{gid}/rules", {"deny": [2, 6]})
check("the owner PC sets rules", st == 200, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/send", {"text": "смотри https://example.org"})
check("the PC itself refuses a banned kind (link)", st == 400, (st, r))
st, r = pc_upload("фото.jpg", b"\xff\xd8\xff" + os.urandom(1000))
check("the PC refuses to attach a banned kind (image)", st == 400, (st, r))
rd = ph.read(gid)
check("the phone sees the rules entry (system, from the owner)", any(k == 7 and b.get("op") == "rules" and sorted(b["deny"]) == [2, 6] for _, k, b in rd), rd)
st, r = ph.call("POST", f"/mobile/groups/{gid}/log", {"epoch": 1, "payload_b64": b64(enc_entry(ph.keys[1], gid, 1, alias, 2, {"n": "x.jpg"}))})
d = a.api("GET", f"/api/groups/{gid}/messages?after=0")[1]
check("an image that sneaks in is hidden on the PC", all(m["kind"] != 2 for m in d["messages"]), d)

# 4) a phone owns a group, the PC asks for an invitation, gets it by code, then reads the feed
st, r = ph.call("POST", "/mobile/groups", {"name": "Клуб телефона", "topic": "клуб", "kind": "invite"})
pg = r["id"]
ph.publish(pg); st, groups = ph.call("GET", "/mobile/groups")
ph.provision(pg)
st, r = a.api("GET", "/api/groups")
disc = [x for x in r["catalog"] if x["id"] == pg]
check("the PC sees the phone's invite-only group among the node's groups", disc and disc[0]["kind"] == "invite", r)
st, r = a.api("POST", f"/api/groups/{pg}/request", {"name": "Хозяин ПК"})
check("the PC asks for an invitation", st == 200, (st, r))
st, r = ph.call("GET", f"/mobile/groups/{pg}/requests")
check("the phone owner sees the request from the PC", st == 200 and r["requests"] and r["requests"][0]["name"] == "Хозяин ПК", (st, r))
rid = r["requests"][0]["id"]
st, inv = ph.call("POST", f"/mobile/groups/{pg}/invite", {"uses": 1})
ph.call("DELETE", f"/mobile/groups/{pg}/requests/{rid}")
st, r = a.api("POST", f"/api/groups/{pg}/join", {"code": inv["code"], "name": "Хозяин ПК"})
check("the PC joins with the invitation code", st == 200, (st, r))
ph.provision(pg)
def pc_has_key():
    st, d = a.api("GET", f"/api/groups/{pg}/messages?after=0")
    return d if st == 200 and d.get("has_key") else None
check("the phone owner seals the key for the PC", wait(lambda: (ph.provision(pg), pc_has_key())[1]) is not None)
ph.write(pg, groups["groups"][[x["id"] for x in groups["groups"]].index(pg)]["my_alias"], 1, {"t": "добро пожаловать, ПК"})
check("the PC reads the phone owner's message", wait(lambda: (lambda d: d if d and any(m["body"].get("t") == "добро пожаловать, ПК" for m in d["messages"]) else None)(a.api("GET", f"/api/groups/{pg}/messages?after=0")[1])) is not None)

# 4b) the PC moderates its own group; invitations to a phone; requests answered, "never remind"
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
ed = Ed25519PrivateKey.generate()
ed_pub = ed.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
x_seed, x_pub = new_keys()
sig = ed.sign(b"YANDI-MOBILE-KEYS-V1\0" + ed_pub + x_pub)
st, r = ph.call("POST", "/mobile/pubkeys", {"ed25519_pub": b64(ed_pub), "x25519_pub": b64(x_pub), "signature": b64(sig)})
check("the phone registers its signed key bundle", st in (200, 204), (st, r))
st, r = a.api("GET", f"/api/groups/{gid}/info")
mem = {m["alias"]: m for m in r["members"]}
check("the PC lists members with roles and announced names", st == 200 and mem[alias]["name"] == "Телефон-Боб" and mem[alias]["role"] == "member", r)
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "mute", "alias": alias})
check("the PC owner mutes a member", st == 200, (st, r))
st, r = ph.write(gid, alias, 1, {"t": "я заглушён"})
check("the muted phone cannot post", st == 403, (st, r))
a.api("POST", f"/api/groups/{gid}/mod", {"action": "unmute", "alias": alias})
st, r = ph.write(gid, alias, 1, {"t": "снова могу"})
check("unmuted, it posts again", st == 200, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "promote", "alias": alias})
check("the PC owner makes the phone a moderator", st == 200, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "demote", "alias": alias})
check("and takes it back", st == 200, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "kick", "alias": "нет такого"})
check("moderating a stranger fails cleanly", st == 400, (st, r))

# 4a) group settings (rename), bans with the banned list, the admin tier and its limits
t_eve = pair_phone("eve")
eve = Phone(t_eve)
st, r = a.api("POST", f"/api/groups/{gid}/settings", {"name": "  "})
check("an empty group name is refused", st == 400, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/settings", {"topic": ""})
check("an empty topic is refused", st == 400, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/settings", {"name": "Болталка", "topic": "Новости", "about": "новое описание"})
check("the owner PC renames the group and changes topic/description", st == 200, (st, r))
st, r = ph.call("GET", "/mobile/groups")
g_ = [x for x in r["groups"] if x["id"] == gid][0]
check("the phone sees the new name, topic and description", g_["name"] == "Болталка" and g_["topic"] == "Новости" and g_["about"] == "новое описание", g_)
st, r = ph.call("POST", f"/mobile/groups/{gid}/settings", {"name": "Взлом"})
check("a plain member cannot change the settings", st == 403, (st, r))
st, r = eve.call("POST", f"/mobile/groups/{gid}/join", {"member_pub": b64(eve.pub)})
eve_alias = r.get("alias")
check("a second phone joins the open group", st == 200 and eve_alias, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "ban", "alias": eve_alias})
check("the PC owner bans a member", st == 200, (st, r))
st, r = a.api("GET", f"/api/groups/{gid}/info")
check("the banned list shows the alias", eve_alias in (r.get("banned") or []), r)
st, r = eve.call("POST", f"/mobile/groups/{gid}/join", {"member_pub": b64(eve.pub)})
check("a banned phone cannot come back", st == 403, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "unban", "alias": eve_alias})
st, r = a.api("GET", f"/api/groups/{gid}/info")
check("after the ban is lifted the list is empty", not r.get("banned"), r)
st, r = eve.call("POST", f"/mobile/groups/{gid}/join", {"member_pub": b64(eve.pub)})
eve_alias = r.get("alias")
check("the phone can join again", st == 200 and eve_alias, (st, r))
# admin tier
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "make_admin", "alias": alias})
check("the PC owner makes the phone an admin", st == 200, (st, r))
st, r = ph.call("GET", "/mobile/groups")
check("the phone sees its admin role", [x for x in r["groups"] if x["id"] == gid][0]["role"] == "admin", r)
st, r = ph.call("POST", f"/mobile/groups/{gid}/settings", {"name": "Болталка 2"})
check("an admin changes the group settings", st == 200, (st, r))
st, r = ph.call("POST", f"/mobile/groups/{gid}/mod", {"action": "mute", "alias": eve_alias})
check("an admin mutes a member", st == 200, (st, r))
st, r = ph.call("POST", f"/mobile/groups/{gid}/mod", {"action": "make_admin", "alias": eve_alias})
check("an admin cannot appoint admins", st == 403, (st, r))
st, r = a.api("GET", f"/api/groups/{gid}/info")
pc_alias = r["group"]["my_alias"]
st, r = ph.call("POST", f"/mobile/groups/{gid}/mod", {"action": "kick", "alias": pc_alias})
check("an admin cannot touch the owner", st == 403, (st, r))
st, r = ph.call("POST", f"/mobile/groups/{gid}/mod", {"action": "promote", "alias": eve_alias})
check("an admin appoints a moderator", st == 200, (st, r))
st, r = eve.call("POST", f"/mobile/groups/{gid}/mod", {"action": "kick", "alias": alias})
check("a moderator cannot touch an admin", st == 403, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "demote", "alias": eve_alias})
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "demote", "alias": alias})
st, r = ph.call("POST", f"/mobile/groups/{gid}/settings", {"name": "Взлом"})
check("after the PC takes the admin role back the phone cannot change settings", st == 403, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/settings", {"name": "Болталка"})

# 4c) handing the group over: PC -> phone, then phone -> another phone; the PC (no longer owner) accepts the new key by the in-group record
got = wait(lambda: (eve.collect(gid), eve.keys)[1])
check("the second phone holds the group key", bool(got), eve.keys)
st, r = a.api("POST", f"/api/groups/{gid}/transfer", {"alias": "нет такого"})
check("a transfer to a stranger is refused", st == 400, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/transfer", {"alias": alias})
check("the PC owner hands the group to the phone", st == 200, (st, r))
st, r = ph.call("GET", "/mobile/groups")
g_ = [x for x in r["groups"] if x["id"] == gid][0]
check("the phone is now the owner and must create a new key", g_["role"] == "owner" and g_["rekey_needed"] is True, g_)
st, r = a.api("GET", f"/api/groups/{gid}/info")
check("the PC became an admin", r["group"]["role"] == "admin", r["group"])
ep = ph.provision(gid)
check("the new owner creates the next key generation", ep >= 2, ep)
ph.write(gid, alias, 1, {"t": "я теперь владелец"})
def pc_reads_new():
    st, d = a.api("GET", f"/api/groups/{gid}/messages?after=0")
    return d if any(m["body"].get("t") == "я теперь владелец" for m in d.get("messages", [])) else None
check("the PC (now admin) reads the new owner's message under the new key", wait(pc_reads_new) is not None)
st, r = a.api("POST", f"/api/groups/{gid}/transfer", {"alias": alias})
check("the PC can no longer hand the group over", st == 400, (st, r))
st, r = a.api("POST", f"/api/groups/{gid}/mod", {"action": "kick", "alias": alias})
check("and cannot touch the new owner", st == 400, (st, r))

# phone -> eve with an in-group handover record; PC pinned the phone's key and has to re-pin
pc_alias2 = a.api("GET", f"/api/groups/{gid}/info")[1]["group"]["my_alias"]
st, mem = ph.call("GET", f"/mobile/groups/{gid}/members")
eve_pub = [m for m in mem["members"] if m["alias"] == eve_alias][0]["member_pub"]
ph.write(gid, alias, 7, {"op": "handover", "to": eve_alias, "pub": eve_pub})
st, r = ph.call("POST", f"/mobile/groups/{gid}/owner", {"alias": "нет такого"})
check("a phone owner cannot hand over to a stranger", st == 404, (st, r))
st, r = ph.call("POST", f"/mobile/groups/{gid}/owner", {"alias": eve_alias})
check("the phone owner hands the group over to the second phone", st == 200, (st, r))
st, r = ph.call("POST", f"/mobile/groups/{gid}/owner", {"alias": eve_alias})
check("a former owner cannot hand over again", st == 403, (st, r))
eve.collect(gid, owner_pub=b64(ph.pub))   # eve is the owner now: she takes the missing old generation with the previous owner's key
ep2 = eve.provision(gid)
check("the second phone creates the next key generation", ep2 >= 3, ep2)
eve.write(gid, eve_alias, 1, {"t": "третий владелец"})
def pc_reads_third():
    st, d = a.api("GET", f"/api/groups/{gid}/messages?after=0")
    return d if d.get("problem") is False and any(m["body"].get("t") == "третий владелец" for m in d.get("messages", [])) else None
check("the PC accepts the new owner's key by the in-group handover record and reads on", wait(pc_reads_third) is not None, a.api("GET", f"/api/groups/{gid}/messages?after=0")[1].get("problem"))

st, r = a.api("POST", "/api/groups/create", {"name": "Приватная", "topic": "личное", "about": "для проверки", "kind": "invite"})
pgid = r["id"]
check("the PC creates an invite-only group", st == 200 and pgid, (st, r))
st, r = a.api("GET", "/api/groups/devices")
check("the PC lists phones it can invite", st == 200 and len(r) >= 1, (st, r))
st, r = a.api("POST", f"/api/groups/{pgid}/invite-code")
check("the PC issues an invitation code", st == 200 and r["invite"].startswith("YANDI-GROUP-1:" + pgid + ":"), (st, r))
code = r["code"]
st, r = ph.call("GET", "/mobile/inbox?since=0&limit=200")
before = len(r) if isinstance(r, list) else len(r.get("items", r.get("messages", [])))
st, r = a.api("POST", f"/api/groups/{pgid}/invite-device", {"peer": a.api("GET", "/api/groups/devices")[1][0]["id"]})
check("the PC sends the invitation to the phone as a personal encrypted message", st == 200, (st, r))
st, r = ph.call("GET", "/mobile/inbox?since=0&limit=200")
after = len(r) if isinstance(r, list) else len(r.get("items", r.get("messages", [])))
check("the message is in the phone's mail queue", after == before + 1, (before, after))
st, r = ph.call("POST", f"/mobile/groups/{pgid}/request", {"name": "Боб"})
check("the phone asks the PC for an invitation", st == 200, (st, r))
st, r = a.api("GET", f"/api/groups/{pgid}/info")
rq = r["requests"]
check("the PC sees the request as a service card", len(rq) == 1 and rq[0]["name"] == "Боб", r)
st, r = a.api("POST", f"/api/groups/{pgid}/answer", {"rid": rq[0]["id"], "accept": True})
check("the PC answers: add (invitation goes to the requester)", st == 200, (st, r))
st, r = ph.call("GET", "/mobile/inbox?since=0&limit=200")
check("a second message reached the phone", (len(r) if isinstance(r, list) else len(r.get("items", r.get("messages", [])))) == after + 1, r)
st, r = ph.call("POST", f"/mobile/groups/{pgid}/request", {"name": "Боб"})
st, r = a.api("GET", f"/api/groups/{pgid}/info")
st, r2 = a.api("POST", f"/api/groups/{pgid}/answer", {"rid": r["requests"][0]["id"], "accept": False, "never": True})
check("the PC answers: never remind", st == 200, (st, r2))
st, r = ph.call("POST", f"/mobile/groups/{pgid}/request", {"name": "Боб снова"})
check("the phone asks again (the node allows it)", st == 200, (st, r))
st, r = a.api("GET", f"/api/groups/{pgid}/info")
check("the PC does not see it again (quietly closed)", r["requests"] == [], r)
st, r = ph.call("POST", f"/mobile/groups/{pgid}/join", {"invite_code": code, "member_pub": b64(ph.pub)})
check("the phone joins with the code the PC issued", st == 200, (st, r))

# 5) the vault on disk is closed
import glob
files = glob.glob(os.path.join(a.dir, "home/.local/share/yandi/groups_pc/*"))
check("the PC keeps its group data in an encrypted vault", len(files) >= 2, files)
blob = b"".join(open(f, "rb").read() for f in files)
check("no plaintext of the messages in the vault", "привет".encode() not in blob and "ответ".encode() not in blob and b"example.org" not in blob)
node_groups = open(os.path.join(a.dir, "home/.local/share/yandi/groups/index.json")).read()
check("the node's shared group store never holds message text", "привет" not in node_groups and "ответ" not in node_groups)
st, r = a.api("POST", f"/api/groups/{pg}/leave")
check("the PC leaves a group and its data is dropped", st == 200 and not os.path.exists(os.path.join(a.dir, f"home/.local/share/yandi/groups_pc/{pg}.log")), (st, r))

a.kill()
print("FAILED:" if fails else "ALL OK", fails or "")
sys.exit(1 if fails else 0)
