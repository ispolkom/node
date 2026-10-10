"""PC/phone file receipt checks for mobiletest; requires Python cryptography.

Run through chaos/mobiletest.py on Claude's socket-enabled test stand.
Uses authenticated E4, file v1 and MsgChannel formats.
"""
import base64
import hashlib
import json
import os
import struct
import time
import sys
from pathlib import Path

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'tests'))
from mobile_e4_reference import seal as seal_e4, open_blob as open_e4


def run(node, req, req_raw, pair_phone, ws_open, ws_send, check):
    from cryptography.hazmat.primitives.asymmetric import x25519, ed25519
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

    b64 = lambda b: base64.b64encode(b).decode()
    raw_pub = lambda k: k.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    token = pair_phone("file receipt test")
    _, info = req("GET", "/mobile/info", token=token)
    peer = info["device_id"]
    x = x25519.X25519PrivateKey.generate()
    ed = ed25519.Ed25519PrivateKey.generate()
    xp, ep = raw_pub(x), raw_pub(ed)
    sig = ed.sign(b"YANDI-MOBILE-KEYS-V1\0" + ep + xp)
    st, _ = req("POST", "/mobile/pubkeys", {
        "x25519_pub": b64(xp), "ed25519_pub": b64(ep), "signature": b64(sig)
    }, token=token)
    check("receipt phone registers signed keys", st == 200)
    _, bundle = req("GET", f"/mobile/pubkey/{node.id}", token=token)
    pc_pub = x25519.X25519PublicKey.from_public_bytes(base64.b64decode(bundle["x25519_pub"]))
    pc_ed = ed25519.Ed25519PublicKey.from_public_bytes(base64.b64decode(bundle["ed25519_pub"]))
    pc_ed.verify(base64.b64decode(bundle["signature"]), b"YANDI-MOBILE-KEYS-V1\0" +
        base64.b64decode(bundle["ed25519_pub"]) + base64.b64decode(bundle["x25519_pub"]))

    def seal(text):
        return seal_e4(text,pc_pub,ed,peer,node.id)

    def open_blob(wire):
        return open_e4(wire,x,pc_ed,node.id,peer)

    sock, _ = ws_open(token)
    def send(text):
        wire = seal(text)
        ws_send(sock, b"\x30" + bytes.fromhex(node.id) + struct.pack("<I", len(wire)) + wire)

    def inbox():
        _, result = req("GET", "/mobile/inbox", token=token)
        return [open_blob(base64.b64decode(m["payload_b64"])) for m in result.get("messages", [])
                if m["from_peer_id"] == node.id]

    def wait_for(predicate):
        end = time.monotonic() + 15
        while time.monotonic() < end:
            result = predicate()
            if result:
                return result
            time.sleep(0.1)
        return None

    def has_receipt(kind, cmid):
        for text in inbox():
            if text.startswith("\x01yandi-rcpt:"):
                value = json.loads(text.split(":", 1)[1])
                if value["kind"] == kind and cmid in value["ids"]:
                    return True
        return False

    data, key = b"file receipt test bytes", os.urandom(32)
    _, transfer = req("POST", "/mobile/files", {
        "to_peer_id": node.id, "file_name": "file", "file_size": len(data), "total_chunks": 1
    }, token=token)
    tid = transfer["transfer_id"]
    nonce = os.urandom(12)
    wire = nonce + AESGCM(key).encrypt(nonce, data, f"yandi-file:v1|{tid}|0|1".encode())
    req_raw("PUT", f"/mobile/files/{tid}/chunk/0", wire, token=token)
    req("POST", f"/mobile/files/{tid}/done", token=token)
    send("\x01yandi-file:" + json.dumps({"v": 1, "tid": tid, "name": "receipt.txt",
        "size": len(data), "chunks": 1, "mime": "text/plain", "key": b64(key),
        "sha": hashlib.sha256(data).hexdigest()}))
    check("phone file gets delivered with file_tid", bool(wait_for(lambda: has_receipt("delivered", "file_" + tid))))
    _, history = node.api("GET", f"/api/chat/history/{peer}")
    check("opening PC chat reads phone file", bool(wait_for(lambda: has_receipt("read", "file_" + tid))))
    attachment = next(m["attachment"] for m in history["messages"] if m.get("attachment"))
    _, sent = node.api("POST", f"/api/chat/send/{peer}", {"text": "", "attachment": attachment})
    check("PC sends received file back", sent.get("status") == "success", sent)

    def pc_offer():
        for text in inbox():
            if text.startswith("\x01yandi-msg:"):
                value = json.loads(text.split(":", 1)[1])
                if value["text"].startswith("\x01yandi-file:"):
                    return value
        return None
    envelope = wait_for(pc_offer)
    check("PC file offer is wrapped with cmid", bool(envelope))
    if envelope:
        offer = json.loads(envelope["text"].split(":", 1)[1])
        st, wire = req_raw("GET", f"/mobile/files/{offer['tid']}/chunk/0", token=token)
        plain = AESGCM(base64.b64decode(offer["key"])).decrypt(
            wire[:12], wire[12:], f"yandi-file:v1|{offer['tid']}|0|1".encode())
        check("wrapped PC offer downloads and decrypts", st == 200 and plain == data)
        for kind, expected in (("delivered", "Delivered"), ("read", "Read"), ("delivered", "Read")):
            send("\x01yandi-rcpt:" + json.dumps({"kind": kind, "ids": [envelope["cmid"]]}))
            def status_matches():
                _, h = node.api("GET", f"/api/chat/history/{peer}")
                return any(m["msg_id"] == envelope["cmid"] and m["status"] == expected for m in h["messages"])
            check("PC file receipt " + kind + " -> " + expected, bool(wait_for(status_matches)))
        req("DELETE", f"/mobile/files/{offer['tid']}", token=token)
    sock.close()
