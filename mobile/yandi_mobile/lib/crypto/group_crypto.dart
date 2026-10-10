import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';

/// Виды записей группы. Метка вида лежит открыто в начале записи (узел её видит и может по ней ограничивать/фильтровать), но входит в
/// проверяемые данные: подменить вид без ключа группы нельзя — расшифровка не пройдёт.
class GroupKind {
  static const text = 1, image = 2, file = 3, audio = 4, video = 5, link = 6, system = 7;
  static const all = [text, image, file, audio, video, link, system];
}

/// Шифрование групп (формат — docs/CLIENT_WIRE.md «Группы»; содержимое узел не читает).
///
/// * Ключ группы — 32 случайных байта, создаёт телефон-владелец. Поколение ключа (epoch) растёт при смене.
/// * Конверт ключа для участника: X25519 между закрытым ключом владельца и открытым ключом участника (оба — свои для этой группы) →
///   HKDF → AES-256-GCM. Так конверт подлинный от владельца (а не от узла, который хранит открытые ключи).
/// * Запись: `[1 версия][1 вид][12 случайное число][шифртекст][16 метка]`; AAD = группа, поколение, псевдоним автора, вид.
class GroupCrypto {
  static const version = 1;
  static final AesGcm _aes = AesGcm.with256bits();
  static final X25519 _x = X25519();

  static Uint8List newGroupKey() {
    final r = Random.secure();
    return Uint8List.fromList(List<int>.generate(32, (_) => r.nextInt(256)));
  }

  /// Пара ключей участника для одной группы: (закрытый seed 32, открытый 32).
  static Future<(Uint8List, Uint8List)> newMemberKeys() async {
    final kp = await _x.newKeyPair();
    final seed = await kp.extractPrivateKeyBytes();
    final pub = (await kp.extractPublicKey()).bytes;
    return (Uint8List.fromList(seed), Uint8List.fromList(pub));
  }

  /// Открытый ключ по закрытому seed (нужен для проверок и совместимости с клиентом на узле).
  static Future<Uint8List> publicFromSeed(List<int> seed) async {
    final kp = await _x.newKeyPairFromSeed(seed);
    return Uint8List.fromList((await kp.extractPublicKey()).bytes);
  }

  static Future<SecretKey> _boxKey(List<int> mySeed, List<int> theirPub, String gid, int epoch) async {
    final kp = await _x.newKeyPairFromSeed(mySeed);
    final shared = await _x.sharedSecretKey(keyPair: kp, remotePublicKey: SimplePublicKey(theirPub, type: KeyPairType.x25519));
    final bytes = await shared.extractBytes();
    if (bytes.every((b) => b == 0)) throw StateError('Invalid X25519 shared secret');
    return Hkdf(hmac: Hmac.sha256(), outputLength: 32).deriveKey(
      secretKey: SecretKey(bytes),
      nonce: utf8.encode('yandi-group-box:v1'),
      info: utf8.encode('$gid|$epoch'),
    );
  }

  static List<int> _boxAad(String gid, int epoch) => utf8.encode('yandi-group-key:v1|$gid|$epoch');

  /// Владелец запечатывает ключ группы для участника.
  static Future<Uint8List> sealKey(List<int> groupKey, {required String gid, required int epoch, required List<int> ownerSeed, required List<int> memberPub}) async {
    final key = await _boxKey(ownerSeed, memberPub, gid, epoch);
    final box = await _aes.encrypt(groupKey, secretKey: key, aad: _boxAad(gid, epoch));
    return Uint8List.fromList(box.concatenation());
  }

  /// Участник достаёт ключ группы. Бросает исключение, если конверт не от владельца с этим открытым ключом.
  static Future<Uint8List> openKey(List<int> wire, {required String gid, required int epoch, required List<int> mySeed, required List<int> ownerPub}) async {
    final key = await _boxKey(mySeed, ownerPub, gid, epoch);
    final box = SecretBox.fromConcatenation(wire, nonceLength: 12, macLength: 16);
    final plain = await _aes.decrypt(box, secretKey: key, aad: _boxAad(gid, epoch));
    if (plain.length != 32) throw StateError('bad group key');
    return Uint8List.fromList(plain);
  }

  static List<int> _aad(String gid, int epoch, String from, int kind) => utf8.encode('yandi-group-entry:v1|$gid|$epoch|$from|$kind');

  /// Содержимое записи — произвольная структура JSON; вид — одна из `GroupKind`.
  static Future<Uint8List> encryptEntry(List<int> groupKey, {required String gid, required int epoch, required String from, required int kind, required Map<String, dynamic> body}) async {
    if (!GroupKind.all.contains(kind)) throw ArgumentError('bad kind');
    // случайный номер записи внутри шифртекста: повтор того же шифртекста (узлом или кем-то ещё) получатель узнаёт и отбрасывает
    final withId = body.containsKey('i') ? body : {...body, 'i': _randomId()};
    final box = await _aes.encrypt(utf8.encode(jsonEncode(withId)), secretKey: SecretKey(groupKey), aad: _aad(gid, epoch, from, kind));
    return Uint8List.fromList([version, kind, ...box.concatenation()]);
  }

  static String _randomId() {
    final r = Random.secure();
    return List.generate(16, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  }

  /// Вид записи без расшифровки (null — формат не наш).
  static int? peekKind(List<int> payload) => payload.length > 2 + 12 + 16 && payload[0] == version && GroupKind.all.contains(payload[1]) ? payload[1] : null;

  static Future<({int kind, Map<String, dynamic> body})> decryptEntry(List<int> groupKey, List<int> payload, {required String gid, required int epoch, required String from}) async {
    final kind = peekKind(payload);
    if (kind == null) throw const FormatException('not a group entry');
    final box = SecretBox.fromConcatenation(payload.sublist(2), nonceLength: 12, macLength: 16);
    final plain = await _aes.decrypt(box, secretKey: SecretKey(groupKey), aad: _aad(gid, epoch, from, kind));
    final j = jsonDecode(utf8.decode(plain));
    if (j is! Map<String, dynamic>) throw const FormatException('bad body');
    return (kind: kind, body: j);
  }
}
