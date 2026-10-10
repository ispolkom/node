import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:cryptography/cryptography.dart';

/// E4: authenticated sender, recipient, kind, random id and signed timestamp.
/// No E2/E3 downgrade. See docs/CLIENT_WIRE.md for the shared Rust/Dart format.
class E2ECrypto {
  static const headerSize = 134;
  static const ttlMs = 30 * 24 * 3600 * 1000;
  static final _domain = utf8.encode('YANDI-E2E-E4\u0000');
  static final _signDomain = utf8.encode('YANDI-E2E-E4-SIGN\u0000');

  static bool isEncrypted(List<int> data) =>
      data.length >= headerSize + 16 + 64 && data[0] == 0xE4;

  static Future<SecretKey> _key(List<int> shared, List<int> header) async {
    if (shared.length != 32 || shared.every((b) => b == 0)) {
      throw StateError('Invalid X25519 shared secret');
    }
    return Hkdf(hmac: Hmac.sha256(), outputLength: 32).deriveKey(
      secretKey: SecretKey(shared),
      nonce: crypto.sha256.convert([..._domain, ...header]).bytes,
      info: utf8.encode('YANDI-E2E-E4-KEY'),
    );
  }

  static List<int> peerBytes(String hex) {
    if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(hex)) {
      throw FormatException('Invalid peer id');
    }
    return List.generate(
        32, (i) => int.parse(hex.substring(i * 2, i * 2 + 2), radix: 16));
  }

  static Future<List<int>> encrypt(
    String text,
    List<int> recipientPub, {
    required String from,
    required String to,
    required int kind,
    required Future<Uint8List> Function(List<int>) sign,
    int? timestampMs,
  }) async {
    if (kind != 1 && kind != 2) throw ArgumentError('Invalid kind');
    final kp = await X25519().newKeyPair();
    final pub = await kp.extractPublicKey();
    final shared = await X25519().sharedSecretKey(
        keyPair: kp,
        remotePublicKey:
            SimplePublicKey(recipientPub, type: KeyPairType.x25519));
    final aes = AesGcm.with256bits();
    final nonce = aes.newNonce();
    final random = Random.secure();
    final ts = ByteData(8)
      ..setUint64(
          0, timestampMs ?? DateTime.now().millisecondsSinceEpoch, Endian.big);
    final header = [
      0xE4,
      kind,
      ...peerBytes(from),
      ...peerBytes(to),
      ...List.generate(16, (_) => random.nextInt(256)),
      ...ts.buffer.asUint8List(),
      ...pub.bytes,
      ...nonce
    ];
    final box = await aes.encrypt(utf8.encode(text),
        secretKey: await _key(await shared.extractBytes(), header),
        nonce: nonce,
        aad: [..._domain, ...header]);
    final wire = [...header, ...box.cipherText, ...box.mac.bytes];
    final signature = await sign([..._signDomain, ...wire]);
    if (signature.length != 64) throw StateError('Invalid signature');
    return [...wire, ...signature];
  }

  static Future<String?> decrypt(
    Uint8List data,
    Future<Uint8List> Function(List<int>) ecdh, {
    required String from,
    required String to,
    required int kind,
    required List<int> senderEd,
    required Future<bool> Function(String id, int expiresMs) claimReplay,
    void Function()? onDuplicate,
    int? nowMs,
  }) async {
    if (!isEncrypted(data)) return null;
    try {
      if (data[1] != kind ||
          (kind != 1 && kind != 2) ||
          !_equal(peerBytes(from), data.sublist(2, 34)) ||
          !_equal(peerBytes(to), data.sublist(34, 66))) return null;
      final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
      final ts = ByteData.sublistView(data, 82, 90).getUint64(0, Endian.big);
      if (ts > now + 300000 || now > ts + ttlMs) return null;
      final end = data.length - 64;
      if (!await Ed25519().verify([..._signDomain, ...data.sublist(0, end)],
          signature: Signature(data.sublist(end),
              publicKey: SimplePublicKey(senderEd, type: KeyPairType.ed25519))))
        return null;
      final header = data.sublist(0, headerSize);
      final key = await _key(await ecdh(data.sublist(90, 122)), header);
      final plain = await AesGcm.with256bits().decrypt(
          SecretBox(data.sublist(headerSize, end - 16),
              nonce: data.sublist(122, 134),
              mac: Mac(data.sublist(end - 16, end))),
          secretKey: key,
          aad: [..._domain, ...header]);
      final text = utf8.decode(plain);
      final id = data
          .sublist(2, 82)
          .map((b) => b.toRadixString(16).padLeft(2, '0'))
          .join();
      if (!await claimReplay(id, ts + ttlMs)) {
        onDuplicate?.call();
        return null;
      }
      return text;
    } catch (_) {
      return null;
    }
  }

  static bool _equal(List<int> a, List<int> b) {
    if (a.length != b.length) return false;
    var difference = 0;
    for (var i = 0; i < a.length; i++) {
      difference |= a[i] ^ b[i];
    }
    return difference == 0;
  }
}
