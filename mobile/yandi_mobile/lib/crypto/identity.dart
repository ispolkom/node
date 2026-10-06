import 'dart:convert';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Криптографическая идентичность пользователя.
///
/// Два независимых ключа:
///   Ed25519  — подпись и идентификация (peer_id = hex публичного ключа)
///   X25519   — ECDH для E2E шифрования сообщений
///
/// Ключи деривируются из пользовательской позиционной фразы через Argon2id.
/// Seed хранится в flutter_secure_storage (Android Keystore).
/// Сама фраза НИКОГДА не сохраняется — только деривированный seed.
class Identity {
  final SimpleKeyPair _edKP;
  final SimpleKeyPair _xKP;

  final Uint8List ed25519PublicBytes;
  final Uint8List x25519PublicBytes;

  Identity._({
    required SimpleKeyPair edKP,
    required SimpleKeyPair xKP,
    required this.ed25519PublicBytes,
    required this.x25519PublicBytes,
  })  : _edKP = edKP,
        _xKP  = xKP;

  /// Peer ID = hex(Ed25519 pub) — одинаковый на всех устройствах при одной фразе
  String get peerId      => _toHex(ed25519PublicBytes);
  String get peerIdShort => peerId.substring(0, 16);

  String get x25519PubBase64  => base64.encode(x25519PublicBytes);
  String get ed25519PubBase64 => base64.encode(ed25519PublicBytes);

  // ── Хранение ───────────────────────────────────────────────────────────────

  static const _store = FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
  );
  static const _kEd = 'id_ed25519_seed';
  static const _kX  = 'id_x25519_seed';

  /// true — seed уже сохранён (пользователь создал аккаунт ранее)
  static Future<bool> hasStoredIdentity() async {
    final s = await _store.read(key: _kEd);
    return s != null;
  }

  /// Загрузить сохранённый seed. Возвращает null если аккаунта нет.
  static Future<Identity?> loadExisting() async {
    final edB64 = await _store.read(key: _kEd);
    final xB64  = await _store.read(key: _kX);
    if (edB64 == null || xB64 == null) return null;
    return _fromSeeds(base64.decode(edB64), base64.decode(xB64));
  }

  /// Загрузить или сгенерировать (обратная совместимость).
  /// Для новых пользователей лучше использовать [fromPhrase].
  static Future<Identity> loadOrGenerate() async {
    final existing = await loadExisting();
    if (existing != null) return existing;
    return _generate();
  }

  // ── Деривация из позиционной фразы ────────────────────────────────────────

  /// Кол-во полей (макс).
  static const int fieldCount = 20;

  /// Деривировать идентичность из позиционной фразы пользователя.
  ///
  /// [fields] — список из [fieldCount] строк. Пустая строка = пустое поле.
  /// Позиция кодируется индексом: "слово" в поле 0 ≠ "слово" в поле 7.
  ///
  /// Алгоритм:
  ///   phrase = fields.join("|")         — позиция закодирована разделителями
  ///   seed64 = Argon2id(phrase, salt)   — 64 байта (memory-hard KDF)
  ///   ed_seed = seed64[0..31]           → Ed25519 keypair → peer_id
  ///   x_seed  = seed64[32..63]          → X25519 keypair  → E2E шифрование
  ///
  /// Сохраняет seed в Keystore — фраза нигде не хранится.
  static Future<Identity> fromPhrase(List<String> fields) async {
    assert(fields.length == fieldCount);

    // Строка с позиционной кодировкой: "слово||||||дурак|||..."
    final phraseStr = fields.join('|');

    // Argon2id: memory-hard, устойчив к GPU/ASIC брутфорсу
    const salt = 'YANDI-identity-v1'; // фиксированный домен, не секрет
    final argon2 = Argon2id(
      memory:      16384, // 16 MB — баланс скорости и безопасности на мобиле
      parallelism: 2,
      iterations:  2,
      hashLength:  64,    // 32 байта на Ed25519 + 32 байта на X25519
    );

    final derived = await argon2.deriveKey(
      secretKey: SecretKey(utf8.encode(phraseStr)),
      nonce:     utf8.encode(salt),
    );
    final seed64 = await derived.extractBytes();

    final edSeed = seed64.sublist(0, 32);
    final xSeed  = seed64.sublist(32, 64);

    // Сохраняем seed в Keystore
    await _store.write(key: _kEd, value: base64.encode(edSeed));
    await _store.write(key: _kX,  value: base64.encode(xSeed));

    return _fromSeeds(edSeed, xSeed);
  }

  /// Проверить фразу без сохранения (для валидации восстановления).
  /// Возвращает peer_id если фраза верна, иначе null.
  static Future<String?> derivePeerId(List<String> fields) async {
    try {
      final id = await fromPhrase(fields);
      return id.peerId;
    } catch (_) {
      return null;
    }
  }

  /// Удалить сохранённый аккаунт (сброс).
  static Future<void> clear() async {
    await _store.delete(key: _kEd);
    await _store.delete(key: _kX);
  }

  // ── Внутренние методы ──────────────────────────────────────────────────────

  static Future<Identity> _generate() async {
    final edKP = await Ed25519().newKeyPair();
    final xKP  = await X25519().newKeyPair();

    final edSeed = await edKP.extractPrivateKeyBytes();
    final xSeed  = await xKP.extractPrivateKeyBytes();

    await _store.write(key: _kEd, value: base64.encode(edSeed));
    await _store.write(key: _kX,  value: base64.encode(xSeed));

    return _build(edKP, xKP);
  }

  static Future<Identity> _fromSeeds(List<int> edSeed, List<int> xSeed) async {
    final edKP = await Ed25519().newKeyPairFromSeed(edSeed);
    final xKP  = await X25519().newKeyPairFromSeed(xSeed);
    return _build(edKP, xKP);
  }

  static Future<Identity> _build(SimpleKeyPair edKP, SimpleKeyPair xKP) async {
    final edPub = await edKP.extractPublicKey();
    final xPub  = await xKP.extractPublicKey();
    return Identity._(
      edKP:               edKP,
      xKP:                xKP,
      ed25519PublicBytes: Uint8List.fromList(edPub.bytes),
      x25519PublicBytes:  Uint8List.fromList(xPub.bytes),
    );
  }

  // ── Подпись ────────────────────────────────────────────────────────────────

  Future<Uint8List> sign(List<int> message) async {
    final sig = await Ed25519().sign(message, keyPair: _edKP);
    return Uint8List.fromList(sig.bytes);
  }

  static Future<bool> verify({
    required List<int> message,
    required List<int> signature,
    required List<int> publicKey,
  }) async {
    try {
      final pub = SimplePublicKey(publicKey, type: KeyPairType.ed25519);
      return await Ed25519().verify(
          message, signature: Signature(signature, publicKey: pub));
    } catch (_) {
      return false;
    }
  }

  // ── ECDH ───────────────────────────────────────────────────────────────────

  Future<Uint8List> ecdh(List<int> theirX25519Pub) async {
    final remote = SimplePublicKey(theirX25519Pub, type: KeyPairType.x25519);
    final shared =
        await X25519().sharedSecretKey(keyPair: _xKP, remotePublicKey: remote);
    return Uint8List.fromList(await shared.extractBytes());
  }

  // ── Utils ──────────────────────────────────────────────────────────────────

  static String _toHex(List<int> bytes) =>
      bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
}
