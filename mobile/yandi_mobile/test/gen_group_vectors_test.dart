import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/group_crypto.dart';

/// Не проверка, а генератор контрольных векторов для Rust-клиента групп на узле (tests: src/group_client.rs).
void main() {
  test('print vectors', () async {
    const gid = '00112233445566778899aabbccddeeff';
    final ownerSeed = Uint8List.fromList(List.generate(32, (i) => i + 1));
    final memberSeed = Uint8List.fromList(List.generate(32, (i) => 100 + i));
    // публичные ключи считаем самой библиотекой: через newKeyPairFromSeed
    final x = (await _pub(ownerSeed), await _pub(memberSeed));
    final key = Uint8List.fromList(List.generate(32, (i) => 200 + i));
    final box = await GroupCrypto.sealKey(key, gid: gid, epoch: 3, ownerSeed: ownerSeed, memberPub: x.$2);
    final entry = await GroupCrypto.encryptEntry(key, gid: gid, epoch: 3, from: 'Тихий Лис-42', kind: GroupKind.text, body: {'t': 'привет 😀'});
    print('VECTOR ${jsonEncode({
      'gid': gid, 'owner_seed': base64.encode(ownerSeed), 'owner_pub': base64.encode(x.$1),
      'member_seed': base64.encode(memberSeed), 'member_pub': base64.encode(x.$2),
      'key': base64.encode(key), 'epoch': 3, 'box': base64.encode(box),
      'from': 'Тихий Лис-42', 'entry': base64.encode(entry), 'text': 'привет 😀',
    })}');
  });
}

Future<Uint8List> _pub(Uint8List seed) async {
  // GroupCrypto не отдаёт публичный ключ по seed — берём тот же X25519
  return GroupCrypto.publicFromSeed(seed);
}
