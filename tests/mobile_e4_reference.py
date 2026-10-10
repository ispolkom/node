"""Independent E4 implementation for cross-language vectors and chaos clients."""
import hashlib
import os
import struct
import time
from cryptography.hazmat.primitives.asymmetric import x25519, ed25519
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

DOMAIN = b"YANDI-E2E-E4\0"
SIGN = b"YANDI-E2E-E4-SIGN\0"

def raw_pub(key):
    return key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)

def derive(shared, header):
    if shared == bytes(32):
        raise ValueError("zero shared secret")
    return HKDF(algorithm=hashes.SHA256(), length=32,
        salt=hashlib.sha256(DOMAIN + header).digest(), info=b"YANDI-E2E-E4-KEY").derive(shared)

def seal(text, recipient_x, signer, sender, recipient, kind=1,
         timestamp=None, ephemeral=None, nonce=None, message_id=None):
    ephemeral = ephemeral or x25519.X25519PrivateKey.generate()
    nonce = nonce or os.urandom(12)
    timestamp = int(time.time()*1000) if timestamp is None else timestamp
    header = bytes([0xe4,kind]) + bytes.fromhex(sender) + bytes.fromhex(recipient)
    header += (message_id or os.urandom(16)) + struct.pack(">Q",timestamp) + raw_pub(ephemeral) + nonce
    wire = header + AESGCM(derive(ephemeral.exchange(recipient_x),header)).encrypt(nonce,text.encode(),DOMAIN+header)
    return wire + signer.sign(SIGN+wire)

def open_blob(wire, secret, signing_pub, sender, recipient, kind=1):
    assert len(wire) >= 214 and wire[:2] == bytes([0xe4,kind])
    assert wire[2:34] == bytes.fromhex(sender) and wire[34:66] == bytes.fromhex(recipient)
    signing_pub.verify(wire[-64:],SIGN+wire[:-64])
    shared = secret.exchange(x25519.X25519PublicKey.from_public_bytes(wire[90:122]))
    return AESGCM(derive(shared,wire[:134])).decrypt(wire[122:134],wire[134:-64],DOMAIN+wire[:134]).decode()

def vector():
    import json
    x = x25519.X25519PrivateKey.from_private_bytes(bytes([1])*32)
    ed = ed25519.Ed25519PrivateKey.from_private_bytes(bytes([2])*32)
    sender,recipient = "03"*32,"04"*32
    wire = seal("привет E4",x.public_key(),ed,sender,recipient,timestamp=2000000000000,
        ephemeral=x25519.X25519PrivateKey.from_private_bytes(bytes([5])*32),nonce=bytes([6])*12,message_id=bytes([7])*16)
    assert open_blob(wire,x,ed.public_key(),sender,recipient) == "привет E4"
    return json.dumps(dict(wire=wire.hex(), sender=sender,recipient=recipient,
        x_secret=(bytes([1])*32).hex(),ed_public=raw_pub(ed).hex(),timestamp=2000000000000,text="привет E4"),ensure_ascii=False)

if __name__ == '__main__':
    print(vector())
