"""Synthetic three-party E4 stand: sender, recipient, hostile forwarding node.

Run with a Python environment containing cryptography:
    python3 -m unittest discover -s tests -p e2e_relay_boundary.py -v
This does not exercise Android lifecycle or a real YANDI transport.
"""

import unittest

from cryptography.exceptions import InvalidSignature, InvalidTag
from cryptography.hazmat.primitives.asymmetric import ed25519, x25519

from mobile_e4_reference import open_blob, seal


class E4RelayBoundary(unittest.TestCase):
    def setUp(self):
        self.sender = ed25519.Ed25519PrivateKey.generate()
        self.recipient = x25519.X25519PrivateKey.generate()
        self.relay = x25519.X25519PrivateKey.generate()
        self.from_id = "11" * 32
        self.to_id = "22" * 32
        self.secret = "synthetic-only message 7391"
        self.wire = seal(
            self.secret, self.recipient.public_key(), self.sender,
            self.from_id, self.to_id,
        )

    def test_relay_cannot_read_or_replace_paired_device_message(self):
        self.assertNotIn(self.secret.encode(), self.wire)
        with self.assertRaises((InvalidTag, ValueError)):
            open_blob(self.wire, self.relay, self.sender.public_key(), self.from_id, self.to_id)
        self.assertEqual(
            open_blob(self.wire, self.recipient, self.sender.public_key(), self.from_id, self.to_id),
            self.secret,
        )
        changed = bytearray(self.wire)
        changed[136] ^= 1
        with self.assertRaises((InvalidSignature, InvalidTag)):
            open_blob(bytes(changed), self.recipient, self.sender.public_key(), self.from_id, self.to_id)

    def test_relay_cannot_rebind_sender_or_recipient(self):
        with self.assertRaises(AssertionError):
            open_blob(self.wire, self.recipient, self.sender.public_key(), "33" * 32, self.to_id)
        with self.assertRaises(AssertionError):
            open_blob(self.wire, self.recipient, self.sender.public_key(), self.from_id, "44" * 32)


if __name__ == "__main__":
    unittest.main()
