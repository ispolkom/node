import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/e2e_crypto.dart';
import 'package:yandi_mobile/crypto/replay_ledger.dart';
import 'package:yandi_mobile/crypto/peer_trust.dart';

void main() {
  late Directory dir;
  setUp(() async {
    dir = await Directory.systemTemp.createTemp('yandi-e4-test-');
  });
  tearDown(() async {
    await dir.delete(recursive: true);
  });

  test('independent Python E4 vector decrypts; every byte is authenticated',
      () async {
    final v = jsonDecode(
            await File('../../tests/mobile_e4_vector.json').readAsString())
        as Map<String, dynamic>;
    Uint8List unhex(String s) => Uint8List.fromList(List.generate(s.length ~/ 2,
        (i) => int.parse(s.substring(i * 2, i * 2 + 2), radix: 16)));
    final wire = unhex(v['wire']);
    final x = await X25519().newKeyPairFromSeed(unhex(v['x_secret']));
    Future<Uint8List> ecdh(List<int> pub) async =>
        Uint8List.fromList(await (await X25519().sharedSecretKey(
                keyPair: x,
                remotePublicKey:
                    SimplePublicKey(pub, type: KeyPairType.x25519)))
            .extractBytes());
    var claims = 0;
    Future<String?> open(Uint8List data,
            {String? from, int kind = 1, int? now, List<int>? ed}) =>
        E2ECrypto.decrypt(data, ecdh,
            from: from ?? v['sender'],
            to: v['recipient'],
            kind: kind,
            senderEd: ed ?? unhex(v['ed_public']),
            nowMs: now ?? v['timestamp'], claimReplay: (id, expiry) async {
          claims++;
          return true;
        });
    expect(await open(wire), v['text']);
    for (var i = 0; i < wire.length; i++) {
      final bad = Uint8List.fromList(wire);
      bad[i] ^= 1;
      expect(await open(bad), isNull, reason: 'byte $i');
    }
    expect(await open(wire, from: '11' * 32), isNull);
    expect(await open(wire, kind: 2), isNull);
    expect(await open(wire, ed: List.filled(32, 0)), isNull);
    expect(await open(wire, now: v['timestamp'] + E2ECrypto.ttlMs + 1), isNull);
    expect(await open(wire, now: v['timestamp'] - 300001), isNull);
    expect(await open(Uint8List.fromList([0xe3, ...wire.skip(1)])), isNull);
    expect(claims, 1,
        reason: 'failed verification must not poison replay state');
  });

  test('signed round trip uses persistent replay protection', () async {
    final ed = await Ed25519().newKeyPairFromSeed(List.filled(32, 9));
    final xp = await X25519().newKeyPairFromSeed(List.filled(32, 8));
    final pub = await xp.extractPublicKey();
    final senderEd = (await ed.extractPublicKey()).bytes;
    final wire = Uint8List.fromList(await E2ECrypto.encrypt(
        'файл 🔒', pub.bytes,
        from: '01' * 32,
        to: '02' * 32,
        kind: 1,
        sign: (data) async => Uint8List.fromList(
            (await Ed25519().sign(data, keyPair: ed)).bytes)));
    final path = File('${dir.path}/ledger.json');
    var duplicate = false;
    Future<String?> open(ReplayLedger ledger) => E2ECrypto.decrypt(
        wire,
        (eph) async => Uint8List.fromList(await (await X25519().sharedSecretKey(
                keyPair: xp,
                remotePublicKey:
                    SimplePublicKey(eph, type: KeyPairType.x25519)))
            .extractBytes()),
        from: '01' * 32,
        to: '02' * 32,
        kind: 1,
        senderEd: senderEd,
        claimReplay: ledger.claim, onDuplicate: () => duplicate = true);
    expect(await open(ReplayLedger(path)), 'файл 🔒');
    expect(duplicate,isFalse);
    expect(await open(ReplayLedger(path)), isNull,
        reason: 'new instance/reconnect reloads disk');
    expect(duplicate,isTrue);
    duplicate = false;
    await path.writeAsString('broken');
    expect(await open(ReplayLedger(path)), isNull);
    expect(duplicate,isFalse,reason:'storage error must not ACK queued mail');
  });

  test(
      'ledger concurrent consumers, expiry, corruption and full capacity fail closed',
      () async {
    final file = File('${dir.path}/ledger.json');
    final outcomes = await Future.wait(List.generate(
        8, (_) => ReplayLedger(file).claim('one', 200, nowMs: 100)));
    expect(outcomes.where((b) => b).length, 1);
    expect(await ReplayLedger(file).claim('one', 400, nowMs: 201), isTrue);
    await file.writeAsString(
        jsonEncode({for (var i = 0; i < 100000; i++) '$i': 400}));
    await expectLater(
        ReplayLedger(file).claim('new', 400, nowMs: 201), throwsStateError);
    expect((jsonDecode(await file.readAsString()) as Map).length, 100000);
    await file.writeAsString('{"bad":"value"}');
    await expectLater(
        ReplayLedger(file).claim('new', 400, nowMs: 201), throwsStateError);
    await expectLater(
        ReplayLedger(File('${dir.path}/missing/ledger'))
            .claim('new', 400, nowMs: 201),
        throwsA(isA<FileSystemException>()));
  });

  test(
      'TOFU persists and rejects changes across instances and concurrent requests',
      () async {
    final file = File('${dir.path}/pin');
    PeerTrust trust() => PeerTrust(
        read: (_) async => await file.exists() ? file.readAsString() : null,
        write: (_, v) => file.writeAsString(v, flush: true));
    await trust().accept('peer', List.filled(32, 1));
    await trust().accept('peer', List.filled(32, 1));
    await expectLater(
        trust().accept('peer', List.filled(32, 2)), throwsStateError);
    expect(await file.readAsString(), base64.encode(List.filled(32, 1)));
    await file.delete();
    final first = trust().accept('peer', List.filled(32, 3));
    final second = trust().accept('peer', List.filled(32, 4));
    await first;
    await expectLater(second, throwsStateError);
  });

  test('TOFU storage failure never reports success', () async {
    final trust = PeerTrust(
        read: (_) async => null,
        write: (_, __) async => throw FileSystemException('disk full'));
    await expectLater(trust.accept('peer', List.filled(32, 1)),
        throwsA(isA<FileSystemException>()));
  });

  test('replay ledger: «seen» reads without writing; the number is taken only by claim', () async {
    final ledger = ReplayLedger(File('${dir.path}/replay.json'));
    final exp = DateTime.now().millisecondsSinceEpoch + 60000;
    expect(await ledger.seen('aa'), isFalse);
    expect(await ledger.seen('aa'), isFalse, reason: 'проверка ничего не занимает');
    expect(await ledger.claim('aa', exp), isTrue);
    expect(await ledger.seen('aa'), isTrue);
    expect(await ledger.claim('aa', exp), isFalse);
  });
}
