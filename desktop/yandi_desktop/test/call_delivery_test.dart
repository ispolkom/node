import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/call_coordinator.dart';
import 'package:yandi_desktop/domain/call_signaling.dart';
import 'calls_test.dart' show Signals, PhoneApi, phoneEvent, phonePeer;

class FastReceiptSignals extends Signals {
  Future<void> Function()? onWatch;
  @override
  Future<void> watch(CallRef c) async {
    await onWatch?.call();
  }
}

void main() {
  testWidgets('receipt during watch prevents starting the retry timer', (
    t,
  ) async {
    final s = FastReceiptSignals();
    final c = CallCoordinator(signaling: s, authorize: () async => true);
    s.onWatch = () => c.handle(
      CallSignal(phonePeer, {
        'v': 1,
        't': 'ringing',
        'cid': 'call0001',
        'video': false,
      }),
    );
    await c.start(phonePeer, phone: true);
    await t.pump(const Duration(seconds: 6));
    expect(s.commands.where((v) => v == 'invite:call0001'), isEmpty);
    c.dispose();
  });

  testWidgets('cancel received before a later poll suppresses delayed invite', (
    t,
  ) async {
    final s = Signals();
    final c = CallCoordinator(signaling: s, authorize: () async => true);
    s.controller.add(
      CallSignal(phonePeer, {
        'v': 1,
        't': 'hangup',
        'cid': 'call0001',
        'video': false,
      }),
    );
    await t.pump();
    s.incoming = [
      const CallRef(phonePeer, 'call0001', phone: true, incoming: true),
    ];
    await c.poll();
    expect(c.phase, CallPhase.idle);
    expect(s.commands.where((v) => v == 'watch'), isEmpty);
    c.dispose();
  });
  testWidgets('lost phone invite retries same cid; receipt stops retries', (
    t,
  ) async {
    final s = Signals();
    final c = CallCoordinator(signaling: s, authorize: () async => true);
    await c.start(phonePeer, phone: true);
    await t.pump(const Duration(seconds: 2));
    expect(s.commands.where((v) => v == 'invite:call0001').length, 1);
    // Wrong recipient / call receipt must not stop delivery.
    await c.handle(
      CallSignal('wrong', {
        'v': 1,
        't': 'ringing',
        'cid': 'call0001',
        'video': false,
      }),
    );
    await t.pump(const Duration(seconds: 2));
    expect(s.commands.where((v) => v == 'invite:call0001').length, 2);
    await c.handle(
      CallSignal(phonePeer, {
        'v': 1,
        't': 'ringing',
        'cid': 'call0001',
        'video': false,
      }),
    );
    await t.pump(const Duration(seconds: 8));
    expect(s.commands.where((v) => v == 'invite:call0001').length, 2);
    c.dispose();
  });

  testWidgets(
    'unacknowledged retries are bounded and cancellation stops them',
    (t) async {
      final s = Signals();
      final c = CallCoordinator(signaling: s, authorize: () async => true);
      await c.start(phonePeer, phone: true);
      for (var i = 0; i < 14; i++) {
        await t.pump(const Duration(seconds: 2));
      }
      expect(s.commands.where((v) => v == 'invite:call0001').length, 10);
      await c.end();
      await t.pump(const Duration(seconds: 10));
      expect(s.commands.where((v) => v == 'invite:call0001').length, 10);
      c.dispose();
    },
  );

  testWidgets(
    'duplicate invites get receipts without restarting incoming call',
    (t) async {
      final s = Signals();
      final c = CallCoordinator(signaling: s, authorize: () async => true);
      const incoming = CallRef(
        phonePeer,
        'call0001',
        phone: true,
        incoming: true,
      );
      await c.receiveCall(incoming);
      s.incoming = [incoming];
      await c.poll();
      expect(s.commands.where((v) => v == 'ringing:call0001').length, 2);
      expect(s.commands.where((v) => v == 'watch').length, 1);
      await c.end();
      await t.pump(const Duration(seconds: 3));
      s.incoming = [incoming];
      await c.poll();
      expect(c.phase, CallPhase.idle);
      c.dispose();
    },
  );

  test(
    'cancel and invite in same API batch never create an incoming window',
    () async {
      final api = PhoneApi();
      final s = DaemonCallSignaling(api);
      await s.poll(); // Establish event cursor.
      api.last = 2;
      api.events = [phoneEvent(1, 'invite'), phoneEvent(2, 'hangup')];
      expect(await s.poll(), isEmpty);
      s.close();
    },
  );
}
