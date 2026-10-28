import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/e2e_crypto.dart';
import 'package:yandi_mobile/crypto/identity.dart';
import 'package:yandi_mobile/crypto/peer_trust.dart';
import 'package:yandi_mobile/crypto/replay_ledger.dart';
import 'package:yandi_mobile/services/storage_service.dart';
import 'package:yandi_mobile/services/ws_service.dart';
import 'package:yandi_mobile/models/call_signal.dart';

class ObservedLedger extends ReplayLedger {
  final queued = Completer<void>();
  ObservedLedger(super.file, {super.seal, super.open});
  @override
  Future<bool> claim(String id, int expiresMs,
      {int? nowMs, bool urgent = false}) {
    final result = super.claim(id, expiresMs, nowMs: nowMs, urgent: urgent);
    if (urgent && !queued.isCompleted) queued.complete();
    return result;
  }
}

class ReceiverIdentity implements Identity {
  final KeyPair key;
  ReceiverIdentity(this.key);
  @override
  Future<Uint8List> ecdh(List<int> remote) async =>
      Uint8List.fromList(await (await X25519().sharedSecretKey(
              keyPair: key,
              remotePublicKey:
                  SimplePublicKey(remote, type: KeyPairType.x25519)))
          .extractBytes());
  @override
  dynamic noSuchMethod(Invocation i) => super.noSuchMethod(i);
}

Future<void> until(bool Function() condition) async {
  for (var i = 0; i < 300 && !condition(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  expect(condition(), isTrue);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late Directory dir;
  late SimpleKeyPair recipient, sender;
  late List<int> recipientPub, senderPub;
  final from = 'a' * 64, to = 'b' * 64;
  Future<Uint8List> frame(String text) async {
    final payload = await E2ECrypto.encrypt(text, recipientPub,
        from: from,
        to: to,
        kind: 2,
        sign: (bytes) async => Uint8List.fromList(
            (await Ed25519().sign(bytes, keyPair: sender)).bytes));
    return (BytesBuilder()
          ..addByte(ftLiveMsg)
          ..add(E2ECrypto.peerBytes(from))
          ..add((ByteData(8)
                ..setInt64(
                    0, DateTime.now().millisecondsSinceEpoch, Endian.little))
              .buffer
              .asUint8List())
          ..add((ByteData(4)..setUint32(0, payload.length, Endian.little))
              .buffer
              .asUint8List())
          ..add(payload))
        .toBytes();
  }

  PeerKeyBundle bundle() => PeerKeyBundle(List<int>.filled(32, 1), senderPub);
  WsService service(Future<PeerKeyBundle?> Function(String) fetch) =>
      WsService(ReceiverIdentity(recipient),
          getPeerBundle: fetch, ownPeerId: () async => to);
  setUp(() async {
    dir = await Directory.systemTemp.createTemp('yandi-call-wire-');
    recipient = await X25519().newKeyPair();
    sender = await Ed25519().newKeyPair();
    recipientPub = (await recipient.extractPublicKey()).bytes;
    senderPub = (await sender.extractPublicKey()).bytes;
    StorageService.replayLedger = ReplayLedger(File('${dir.path}/replay.json'));
  });
  tearDown(() async {
    StorageService.replayLedger = null;
    await dir.delete(recursive: true);
  });
  final invite =
      '\u0001yandi-call:{"v":1,"t":"invite","cid":"testcall0001","video":false,"ts":${DateTime.now().millisecondsSinceEpoch}}';

  test('fresh encrypted retransmission recovers a transient key lookup failure',
      () async {
    var fetches = 0;
    final ws = service((_) async => ++fetches == 1 ? null : bundle());
    final events = <LiveSignalEvent>[];
    ws.liveStream.listen(events.add);
    final wire = await frame(invite);
    await ws.debugFeed(wire);
    await until(() => ws.debugLiveInFlight == 0);
    expect(events, isEmpty);
    final retransmission = await frame(invite);
    await ws.debugFeed(retransmission);
    await until(() => events.length == 1);
    expect(events.single.text, invite);
    expect(CallSignal.tryParse(events.single.text)?.fresh, isTrue);
    await ws.debugFeed(retransmission);
    await until(() => ws.debugLiveInFlight == 0);
    expect(events.length, 1,
        reason: 'Replay protection remains effective after recovery');
    ws.dispose();
  });

  test('bounded live verification recovers on retransmission after saturation',
      () async {
    final gate = Completer<PeerKeyBundle?>();
    final ws = service((_) => gate.future);
    final events = <LiveSignalEvent>[];
    ws.liveStream.listen(events.add);
    for (var i = 0; i < 8; i++) {
      unawaited(ws.debugFeed(await frame('\u0001yandi-cap:$i')));
    }
    await until(() => ws.debugLiveInFlight == 8);
    expect(ws.debugLiveInFlight, 8);
    final wire = await frame(invite);
    unawaited(ws.debugFeed(wire));
    gate.complete(bundle());
    await until(() => ws.debugLiveInFlight == 0);
    expect(events.where((e) => e.text == invite), isEmpty);
    await ws.debugFeed(wire);
    await until(() => events.any((e) => e.text == invite));
    ws.dispose();
  });

  test(
      'authenticated live invite passes pending history without bypassing replay protection',
      () async {
    final entered = Completer<void>(),
        release = Completer<void>(),
        historyRelease = Completer<void>();
    var writes = 0;
    final ledger = ObservedLedger(File('${dir.path}/shared.json'),
        seal: (json) async {
          writes++;
          if (writes == 1) {
            entered.complete();
            await release.future;
          } else if (writes > 2) {
            await historyRelease.future;
          }
          return json;
        },
        open: (stored) async => stored.isEmpty ? '{}' : stored);
    StorageService.replayLedger = ledger;
    final exp = DateTime.now().millisecondsSinceEpoch + 60000;
    final pending = <Future<bool>>[ledger.claim('chat-0', exp)];
    await entered.future;
    for (var i = 1; i <= 30; i++) {
      pending.add(ledger.claim('chat-$i', exp));
    }
    final ws = service((_) async => bundle());
    final events = <LiveSignalEvent>[];
    ws.liveStream.listen(events.add);
    unawaited(ws.debugFeed(await frame(invite)));
    await ledger.queued.future.timeout(const Duration(seconds: 5));
    expect(events, isEmpty);
    expect(ws.debugLiveInFlight, 1);
    release.complete();
    await until(() => events.length == 1);
    expect(writes, lessThanOrEqualTo(3),
        reason: 'Live delivery precedes the thirty pending history writes');
    historyRelease.complete();
    await Future.wait(pending);
    expect(writes, 32);
    expect(events.single.text, invite);
    ws.dispose();
  });
}
