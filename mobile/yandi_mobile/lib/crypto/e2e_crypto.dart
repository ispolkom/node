import 'dart:convert';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';

/// E2E / X25519 + AES-256-GCM
///
/// Формат зашифрованного blob:
///   [1B magic=0xE2][32B ephemeral_pub][12B nonce][ciphertext+16B tag]
///
/// Отправитель генерирует ephemeral X25519 keypair,
/// вычисляет ECDH с публичным ключом получателя,
/// шифрует AES-256-GCM(shared_secret, nonce, plaintext).
///
/// Получатель видит ephemeral_pub, вычисляет ECDH своим приватным ключом
/// и расшифровывает.
class E2ECrypto {
  static const _magic = 0xE2;

  static bool isEncrypted(List<int> data) =>
      data.isNotEmpty && data[0] == _magic && data.length > 45;

  /// Зашифровать [text] для получателя с публичным X25519 ключом [recipientPub].
  static Future<List<int>> encrypt(
      String text, List<int> recipientPub) async {
    // 1. Ephemeral keypair
    final ephemeralKP = await X25519().newKeyPair();
    final ephemeralPub = await ephemeralKP.extractPublicKey();

    // 2. ECDH → shared secret
    final remoteKey = SimplePublicKey(recipientPub, type: KeyPairType.x25519);
    final sharedSecret = await X25519()
        .sharedSecretKey(keyPair: ephemeralKP, remotePublicKey: remoteKey);
    final sharedBytes = await sharedSecret.extractBytes();

    // 3. AES-256-GCM encrypt
    final aes    = AesGcm.with256bits();
    final sk     = SecretKey(sharedBytes);
    final nonce  = aes.newNonce();
    final box    = await aes.encrypt(
      utf8.encode(text),
      secretKey:  sk,
      nonce:      nonce,
    );

    // 4. Сборка: magic + ephemeral_pub(32) + nonce(12) + ciphertext+tag
    final buf = BytesBuilder();
    buf.addByte(_magic);
    buf.add(ephemeralPub.bytes);   // 32 bytes
    buf.add(nonce);                // 12 bytes
    buf.add(box.cipherText);
    buf.add(box.mac.bytes);        // 16 bytes GCM tag
    return buf.toBytes();
  }

  /// Расшифровать blob.
  /// [ecdhFn] — callback вызывает ECDH приватным ключом получателя:
  ///   (theirPub) → sharedSecret bytes
  static Future<String?> decrypt(
    Uint8List data,
    Future<Uint8List> Function(List<int> theirPub) ecdhFn,
  ) async {
    if (!isEncrypted(data)) return null;
    try {
      final ephemeralPub = data.sublist(1, 33);
      final nonce        = data.sublist(33, 45);
      final cipherAndTag = data.sublist(45);

      final sharedBytes = await ecdhFn(ephemeralPub);

      final aes = AesGcm.with256bits();
      final sk  = SecretKey(sharedBytes);

      final cipherText = cipherAndTag.sublist(0, cipherAndTag.length - 16);
      final tag        = cipherAndTag.sublist(cipherAndTag.length - 16);

      final box = SecretBox(cipherText, nonce: nonce, mac: Mac(tag));
      final plain = await aes.decrypt(box, secretKey: sk);
      return utf8.decode(plain);
    } catch (_) {
      return null;
    }
  }
}
