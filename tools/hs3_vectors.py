#!/usr/bin/env python3
"""Независимый вектор для рукопожатия v3 (docs/HANDSHAKE_HARDENING.md, раздел 3а): только стандартная библиотека Python.
Ed25519 (RFC 8032), X25519 (RFC 7748), HKDF и HMAC-SHA256 написаны здесь заново и проверяются эталонными векторами RFC; Rust-реализация
(`src/hs3`) сверяется с получившимся `tests/hs3_vectors.json`. Запуск: python3 tools/hs3_vectors.py > tests/hs3_vectors.json"""
import hashlib, hmac, json, sys

# ---- X25519 (RFC 7748)
P = 2**255 - 19
def x25519(k: bytes, u: bytes) -> bytes:
    k = bytearray(k); k[0] &= 248; k[31] &= 127; k[31] |= 64
    kn = int.from_bytes(k, "little")
    x1 = int.from_bytes(u, "little") & ((1 << 255) - 1)
    x2, z2, x3, z3, swap = 1, 0, x1, 1, 0
    for t in reversed(range(255)):
        kt = (kn >> t) & 1
        swap ^= kt
        if swap: x2, x3, z2, z3 = x3, x2, z3, z2
        swap = kt
        A = (x2 + z2) % P; AA = A * A % P; B = (x2 - z2) % P; BB = B * B % P; E = (AA - BB) % P
        C = (x3 + z3) % P; D = (x3 - z3) % P; DA = D * A % P; CB = C * B % P
        x3 = (DA + CB) ** 2 % P; z3 = x1 * ((DA - CB) ** 2 % P) % P
        x2 = AA * BB % P; z2 = E * (AA + 121665 * E) % P
    if swap: x2, x3, z2, z3 = x3, x2, z3, z2
    return (x2 * pow(z2, P - 2, P) % P).to_bytes(32, "little")

# ---- Ed25519 (RFC 8032)
Q = 2**252 + 27742317777372353535851937790883648493
def inv(x): return pow(x, P - 2, P)
D = -121665 * inv(121666) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)
def recover_x(y, sign):
    x2 = (y * y - 1) * inv(D * y * y + 1) % P
    if x2 == 0: return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P: x = x * SQRT_M1 % P
    if (x * x - x2) % P: return None
    return P - x if (x & 1) != sign else x
GY = 4 * inv(5) % P
G = (recover_x(GY, 0), GY, 1, recover_x(GY, 0) * GY % P)
def padd(a, b):
    A = (a[1] - a[0]) * (b[1] - b[0]) % P; B = (a[1] + a[0]) * (b[1] + b[0]) % P
    C = 2 * a[3] * b[3] * D % P; Dd = 2 * a[2] * b[2] % P
    E, F, Gg, H = B - A, Dd - C, Dd + C, B + A
    return (E * F % P, Gg * H % P, F * Gg % P, E * H % P)
def pmul(s, pt):
    r = (0, 1, 1, 0)
    while s > 0:
        if s & 1: r = padd(r, pt)
        pt = padd(pt, pt); s >>= 1
    return r
def pcompress(pt):
    zi = inv(pt[2]); x = pt[0] * zi % P; y = pt[1] * zi % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")
def expand(seed):
    h = hashlib.sha512(seed).digest(); a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8; a |= 1 << 254
    return a, h[32:]
def ed_pub(seed): return pcompress(pmul(expand(seed)[0], G))
def ed_sign(seed, msg):
    a, prefix = expand(seed); A = pcompress(pmul(a, G))
    r = int.from_bytes(hashlib.sha512(prefix + msg).digest(), "little") % Q
    R = pcompress(pmul(r, G))
    h = int.from_bytes(hashlib.sha512(R + A + msg).digest(), "little") % Q
    return R + int.to_bytes((r + h * a) % Q, 32, "little")

# ---- HKDF / HMAC
def hmac256(k, m): return hmac.new(k, m, hashlib.sha256).digest()
def hkdf_extract(salt, ikm): return hmac256(salt, ikm)
def hkdf_expand32(prk, info): return hmac256(prk, info + b"\x01")

def selfcheck():
    assert ed_pub(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")).hex() == "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    assert ed_sign(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"), b"").hex() == "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    assert x25519(bytes.fromhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"), bytes.fromhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")).hex() == "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
selfcheck()

# ---- формат (docs/HANDSHAKE_HARDENING.md, 3а)
def node_id(pub): return hashlib.sha256(pub).digest()[:28] + b"YB1\x00"
def header(typ, ctx): return b"YHS3" + bytes([3, typ, 1, ctx, 0])
def lp(s): b = s.encode(); assert len(b) < 256; return bytes([len(b)]) + b
CTX = 1
i_seed, r_seed = bytes([1]) * 32, bytes([2]) * 32
i_pub, r_pub = ed_pub(i_seed), ed_pub(r_seed)
i_id, r_id = node_id(i_pub), node_id(r_pub)
ea_sec, eb_sec = bytes([0x11]) * 32, bytes([0x22]) * 32
nine = (9).to_bytes(32, "little")
ea, eb = x25519(ea_sec, nine), x25519(eb_sec, nine)
req_body = (header(1, CTX) + i_id + i_pub + ea + (1_700_000_000_000).to_bytes(8, "big") + bytes([0xAA]) * 16 + r_id
            + (0x0F).to_bytes(4, "big") + lp("RU") + lp("alice"))
R = req_body + ed_sign(i_seed, b"yandi-hs-v3-req" + req_body)
hR = hashlib.sha256(R).digest()
ack_body = (header(2, CTX) + r_id + r_pub + eb + (1_700_000_001_000).to_bytes(8, "big") + bytes([0xBB]) * 16 + hR + i_id
            + (0x03).to_bytes(4, "big") + lp("NL") + lp("bob"))
A = ack_body + ed_sign(r_seed, b"yandi-hs-v3-ack" + ack_body)
T = hashlib.sha256(b"yandi-hs-v3" + len(R).to_bytes(2, "big") + R + len(A).to_bytes(2, "big") + A).digest()
dh = x25519(ea_sec, eb); assert dh == x25519(eb_sec, ea)
prk = hkdf_extract(T, dh)
def label(l): return hkdf_expand32(prk, b"yandi-hs-v3/" + l + b"\x00" + bytes([CTX]))
i2r, r2i, kc_i, kc_r, exporter = label(b"i2r"), label(b"r2i"), label(b"kc_i"), label(b"kc_r"), label(b"exporter")
fin_i = header(3, CTX) + T + hmac256(kc_i, b"fin-i" + T)
fin_r = header(4, CTX) + T + hmac256(kc_r, b"fin-r" + T)
file_id = b"file-0001"
prk_f = hkdf_extract(b"yandi-file-v1", exporter)
file_key = hkdf_expand32(prk_f, b"file/" + bytes([len(file_id)]) + file_id)
out = {
    "doc": "docs/HANDSHAKE_HARDENING.md 3а; получено независимой реализацией tools/hs3_vectors.py (стандартная библиотека Python)",
    "context": CTX,
    "initiator_seed": i_seed.hex(), "responder_seed": r_seed.hex(),
    "ea_secret": ea_sec.hex(), "eb_secret": eb_sec.hex(),
    "initiator_pub": i_pub.hex(), "responder_pub": r_pub.hex(), "initiator_id": i_id.hex(), "responder_id": r_id.hex(),
    "ea": ea.hex(), "eb": eb.hex(),
    "request": R.hex(), "ack": A.hex(), "h_r": hR.hex(), "transcript": T.hex(), "dh": dh.hex(), "prk": prk.hex(),
    "i2r": i2r.hex(), "r2i": r2i.hex(), "kc_i": kc_i.hex(), "kc_r": kc_r.hex(), "exporter": exporter.hex(),
    "fin_i": fin_i.hex(), "fin_r": fin_r.hex(), "file_id": file_id.hex(), "file_key": file_key.hex(),
    "sizes": {"request": len(R), "ack": len(A), "fin": len(fin_i)},
}
json.dump(out, sys.stdout, indent=2)
print()
