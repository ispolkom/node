#!/usr/bin/env python3
"""Независимый вектор для запечатанного конверта YSE1 (src/crypto/sealed.rs, docs/SIMPLE_DESIGN.md): только стандартная библиотека Python.
X25519 (RFC 7748), Ed25519 (RFC 8032), HKDF-SHA256 и ChaCha20-Poly1305 (RFC 8439) написаны здесь заново и проверяются эталонными
векторами RFC; Rust-реализация сверяется с получившимся `tests/yse_vectors.json`.
Запуск: python3 tools/yse_vectors.py > tests/yse_vectors.json"""
import hashlib, hmac, json, struct

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

def hmac256(k, m): return hmac.new(k, m, hashlib.sha256).digest()
def hkdf32(salt, ikm, info): return hmac256(hmac256(salt, ikm), info + b"\x01")

# ---- ChaCha20-Poly1305 (RFC 8439)
M32 = 0xffffffff
def rotl(x, n): return ((x << n) & M32) | (x >> (32 - n))
def qr(s, a, b, c, d):
    s[a] = (s[a] + s[b]) & M32; s[d] = rotl(s[d] ^ s[a], 16)
    s[c] = (s[c] + s[d]) & M32; s[b] = rotl(s[b] ^ s[c], 12)
    s[a] = (s[a] + s[b]) & M32; s[d] = rotl(s[d] ^ s[a], 8)
    s[c] = (s[c] + s[d]) & M32; s[b] = rotl(s[b] ^ s[c], 7)
def chacha_block(key, counter, nonce):
    st = list(struct.unpack("<4I", b"expand 32-byte k")) + list(struct.unpack("<8I", key)) + [counter] + list(struct.unpack("<3I", nonce))
    w = st[:]
    for _ in range(10):
        qr(w, 0, 4, 8, 12); qr(w, 1, 5, 9, 13); qr(w, 2, 6, 10, 14); qr(w, 3, 7, 11, 15)
        qr(w, 0, 5, 10, 15); qr(w, 1, 6, 11, 12); qr(w, 2, 7, 8, 13); qr(w, 3, 4, 9, 14)
    return struct.pack("<16I", *[(w[i] + st[i]) & M32 for i in range(16)])
def chacha_xor(key, counter, nonce, data):
    out = bytearray()
    for i in range(0, len(data), 64):
        ks = chacha_block(key, counter + i // 64, nonce)
        out += bytes(a ^ b for a, b in zip(data[i:i + 64], ks))
    return bytes(out)
def poly1305(key, msg):
    r = int.from_bytes(key[:16], "little") & 0x0ffffffc0ffffffc0ffffffc0fffffff
    s = int.from_bytes(key[16:], "little"); p = (1 << 130) - 5; acc = 0
    for i in range(0, len(msg), 16):
        n = int.from_bytes(msg[i:i + 16] + b"\x01", "little"); acc = (acc + n) * r % p
    return ((acc + s) & ((1 << 128) - 1)).to_bytes(16, "little")
def pad16(b): return b + b"\x00" * (-len(b) % 16)
def aead_encrypt(key, nonce, pt, aad):
    otk = chacha_block(key, 0, nonce)[:32]
    ct = chacha_xor(key, 1, nonce, pt)
    mac = poly1305(otk, pad16(aad) + pad16(ct) + struct.pack("<QQ", len(aad), len(ct)))
    return ct + mac

def selfcheck():
    assert ed_pub(bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")).hex() == "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    assert x25519(bytes.fromhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"), bytes.fromhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")).hex() == "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
    # RFC 8439, 2.8.2
    key = bytes(range(0x80, 0xa0)); nonce = bytes.fromhex("070000004041424344454647")
    aad = bytes.fromhex("50515253c0c1c2c3c4c5c6c7")
    pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it."
    out = aead_encrypt(key, nonce, pt, aad)
    assert out[-16:].hex() == "1ae10b594f09e26a7e902ecbd0600691", out[-16:].hex()
    assert out[:16].hex() == "d31a8d34648e60db7b86afbc53ef7ec2"
selfcheck()

# ---- конверт YSE1
def ed_to_montgomery(pub):
    y = int.from_bytes(pub, "little") & ((1 << 255) - 1)
    return ((1 + y) * inv((1 - y) % P) % P).to_bytes(32, "little")
def x_secret_of(seed):
    h = hashlib.sha512(seed).digest()[:32]
    return h  # x25519() сама применяет обрезку
def seal(sender_seed, recipient_seed, eph_secret, nonce, created_ms, from_id, to_id, plaintext):
    recipient_pub = ed_pub(recipient_seed)
    recipient_x = ed_to_montgomery(recipient_pub)
    base = (9).to_bytes(32, "little")
    eph_pub = x25519(eph_secret, base)
    shared = x25519(eph_secret, recipient_x)
    key = hkdf32(eph_pub + recipient_x, shared, b"yandi-sealed-v1" + from_id + to_id)
    aad = b"YSE1" + eph_pub + nonce + struct.pack(">Q", created_ms) + from_id + to_id
    ct = aead_encrypt(key, nonce, plaintext, aad)
    sig = ed_sign(sender_seed, b"yandi-sealed-sig-v1" + aad + ct)
    env = b"YSE1" + eph_pub + nonce + struct.pack(">Q", created_ms) + struct.pack(">I", len(ct)) + ct + sig
    return env, dict(recipient_x=recipient_x, eph_pub=eph_pub, shared=shared, key=key, aad=aad, ct=ct, sig=sig)

cases = []
for name, (ss, rs, es, nn, ts, fi, ti, pt) in {
    "text": (bytes([1]) * 32, bytes([2]) * 32, bytes([3]) * 32, bytes([4]) * 12, 1_800_000_000_000, bytes([1]) * 32, bytes([2]) * 32, "привет, мир".encode()),
    "empty": (bytes([5]) * 32, bytes([6]) * 32, bytes([7]) * 32, bytes(range(12)), 1_700_000_000_123, bytes([9]) * 32, bytes([10]) * 32, b""),
    "long": (bytes([11]) * 32, bytes([12]) * 32, bytes([13]) * 32, bytes([255]) * 12, 1_900_000_000_000, bytes([21]) * 32, bytes([22]) * 32, bytes(range(256)) * 5),
}.items():
    env, d = seal(ss, rs, es, nn, ts, fi, ti, pt)
    cases.append({
        "name": name,
        "sender_seed": ss.hex(), "recipient_seed": rs.hex(), "eph_secret": es.hex(), "nonce": nn.hex(),
        "created_ms": ts, "from": fi.hex(), "to": ti.hex(), "plaintext": pt.hex(),
        "recipient_x": d["recipient_x"].hex(), "eph_pub": d["eph_pub"].hex(), "key": d["key"].hex(), "envelope": env.hex(),
    })
print(json.dumps({"format": "YSE1", "cases": cases}, indent=1))
