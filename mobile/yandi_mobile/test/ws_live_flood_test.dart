import 'dart:async';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/identity.dart';
import 'package:yandi_mobile/crypto/peer_trust.dart';
import 'package:yandi_mobile/services/ws_service.dart';

class _FakeIdentity implements Identity {
  @override
  dynamic noSuchMethod(Invocation i) => null;
}

/// Кадр FT_LIVE_MSG, похожий на E4 по первому байту и длине, но без настоящей подписи.
Uint8List _fakeLive(int n) {
  final payload = Uint8List(400)..[0] = 0xE4;
  final b = BytesBuilder()
    ..addByte(ftLiveMsg)
    ..add(Uint8List(32)..[0] = 1)
    ..add((ByteData(8)..setInt64(0, 1000 + n, Endian.little)).buffer.asUint8List())
    ..add((ByteData(4)..setUint32(0, payload.length, Endian.little)).buffer.asUint8List())
    ..add(payload);
  return b.toBytes();
}

void main() {
  test('поток поддельных live-кадров не запускает больше восьми проверок сразу', () async {
    var calls = 0;
    final gate = Completer<PeerKeyBundle?>();
    final ws = WsService(_FakeIdentity(), getPeerBundle: (_) { calls++; return gate.future; }, ownPeerId: () async => 'self');
    for (var i = 0; i < 200; i++) {
      unawaited(ws.debugFeed(_fakeLive(i)));
    }
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(calls, lessThanOrEqualTo(8));
    expect(ws.debugLiveInFlight, lessThanOrEqualTo(8));
    gate.complete(null);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(ws.debugLiveInFlight, 0, reason: 'после ответа счётчик освобождается');
  });
}
