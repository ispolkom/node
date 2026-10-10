import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/group_crypto.dart';

void main() {
  const gid = 'aabbccddeeff00112233445566778899';

  test('конверт ключа: открывает только участник от своего владельца', () async {
    final (oSeed, oPub) = await GroupCrypto.newMemberKeys();
    final (mSeed, mPub) = await GroupCrypto.newMemberKeys();
    final (eSeed, _) = await GroupCrypto.newMemberKeys();
    final (fSeed, fPub) = await GroupCrypto.newMemberKeys();
    final key = GroupCrypto.newGroupKey();
    final box = await GroupCrypto.sealKey(key, gid: gid, epoch: 3, ownerSeed: oSeed, memberPub: mPub);
    expect(await GroupCrypto.openKey(box, gid: gid, epoch: 3, mySeed: mSeed, ownerPub: oPub), key);
    expect(() => GroupCrypto.openKey(box, gid: gid, epoch: 3, mySeed: eSeed, ownerPub: oPub), throwsA(anything), reason: 'чужой');
    expect(() => GroupCrypto.openKey(box, gid: gid, epoch: 3, mySeed: mSeed, ownerPub: fPub), throwsA(anything), reason: 'подменённый ключ владельца (узел)');
    expect(() => GroupCrypto.openKey(box, gid: gid, epoch: 4, mySeed: mSeed, ownerPub: oPub), throwsA(anything), reason: 'другое поколение');
    expect(() => GroupCrypto.openKey(box, gid: 'ff' * 16, epoch: 3, mySeed: mSeed, ownerPub: oPub), throwsA(anything), reason: 'другая группа');
    expect(fSeed.length, 32);
  });

  test('запись: туда-обратно, вид виден, подмены отвергаются', () async {
    final key = GroupCrypto.newGroupKey();
    final p = await GroupCrypto.encryptEntry(key, gid: gid, epoch: 1, from: 'Тихий Лис-42', kind: GroupKind.text, body: {'t': 'привет 😀'});
    expect(GroupCrypto.peekKind(p), GroupKind.text);
    final r = await GroupCrypto.decryptEntry(key, p, gid: gid, epoch: 1, from: 'Тихий Лис-42');
    expect(r.kind, GroupKind.text);
    expect(r.body['t'], 'привет 😀');
    expect(() => GroupCrypto.decryptEntry(key, p, gid: gid, epoch: 1, from: 'Другой-1'), throwsA(anything), reason: 'автор подменён');
    expect(() => GroupCrypto.decryptEntry(key, p, gid: gid, epoch: 2, from: 'Тихий Лис-42'), throwsA(anything), reason: 'поколение');
    expect(() => GroupCrypto.decryptEntry(GroupCrypto.newGroupKey(), p, gid: gid, epoch: 1, from: 'Тихий Лис-42'), throwsA(anything), reason: 'чужой ключ');
    final swapped = [...p]..[1] = GroupKind.image;
    expect(() => GroupCrypto.decryptEntry(key, swapped, gid: gid, epoch: 1, from: 'Тихий Лис-42'), throwsA(anything), reason: 'вид подменён узлом');
    final flipped = [...p]..[p.length - 1] ^= 1;
    expect(() => GroupCrypto.decryptEntry(key, flipped, gid: gid, epoch: 1, from: 'Тихий Лис-42'), throwsA(anything));
    expect(GroupCrypto.peekKind([1, 9, 0]), isNull);
  });
}
