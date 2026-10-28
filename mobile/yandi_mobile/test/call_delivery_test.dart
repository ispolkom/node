import 'dart:async';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/call_signal.dart';
import 'package:yandi_mobile/services/call_service.dart';
import 'package:audioplayers/audioplayers.dart';

// These tests exercise signaling; native sound playback has its own suite.
class SilentPlayer implements AudioPlayer {
  @override
  Future<void> stop() async {}
  @override
  Future<void> dispose() async {}
  @override
  dynamic noSuchMethod(Invocation i) => super.noSuchMethod(i);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() {
    for (final name in [
      'xyz.luan/audioplayers',
      'xyz.luan/audioplayers.global',
      'xyz.luan/audioplayers.global/events',
      'com.cloudwebrtc.webrtc/method'
    ]) {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(MethodChannel(name), (_) async => null);
    }
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMessageHandler(
            'dev.flutter.pigeon.wakelock_plus_platform_interface.WakelockPlusApi.toggle',
            (_) async =>
                const StandardMessageCodec().encodeMessage(<Object?>[]));
  });
  CallService make(Future<bool> Function(String, String) send,
          {void Function()? show}) =>
      CallService(
        ringPlayer: SilentPlayer(),
        soundEnabled: (_) => false,
        sendSignal: send,
        fetchTurn: () async => null,
        nodeHost: () => null,
        nameOf: (p) => p,
        showScreen: show ?? () {},
        onIncomingNotify: (_, __, ___) {},
        onIncomingCancel: () {},
        onLog: (_, __, ___) {},
      );
  testWidgets('duplicate invite re-acknowledges without reopening call',
      (t) async {
    final sent = <CallSignal>[];
    var screens = 0;
    final c = make((_, text) async {
      sent.add(CallSignal.tryParse(text)!);
      return true;
    }, show: () => screens++);
    final invite = CallSignal.now('invite', 'call0001', video: true);
    await c.onSignal('peer', invite);
    await c.onSignal('peer', invite);
    expect(screens, 1);
    expect(sent.map((s) => s.type), ['ringing', 'ringing']);
    expect(sent.every((s) => s.cid == invite.cid && s.video), isTrue);
    c.dispose();
    await t.pump();
  });
  testWidgets('cancel arriving before delayed invite prevents ghost ringing',
      (t) async {
    var screens = 0;
    final c = make((_, __) async => true, show: () => screens++);
    await c.onSignal('peer', CallSignal.now('hangup', 'call0001'));
    await c.onSignal('peer', CallSignal.now('invite', 'call0001'));
    expect(c.phase, CallPhase.idle);
    expect(screens, 0);
    // A distinct, fresh call is still accepted.
    await c.onSignal('peer', CallSignal.now('invite', 'call0002'));
    expect(c.phase, CallPhase.ringing);
    c.dispose();
    await t.pump();
  });
  testWidgets('outgoing retries stop on receipt and preserve cid and video',
      (t) async {
    final sent = <CallSignal>[];
    final c = make((_, text) async {
      sent.add(CallSignal.tryParse(text)!);
      return true;
    });
    await c.start('peer', withVideo: true);
    await t.pump(const Duration(seconds: 2));
    expect(sent.length, 2);
    expect(sent.every((s) => s.type == 'invite' && s.cid == c.cid && s.video),
        isTrue);
    await c.onSignal('peer', CallSignal.now('ringing', c.cid!, video: true));
    await t.pump(const Duration(seconds: 10));
    expect(sent.length, 2);
    c.dispose();
    await t.pump();
  });
  testWidgets('cancel while first send waits cannot restart dialing or retries',
      (t) async {
    final gate = Completer<bool>();
    final sent = <CallSignal>[];
    final c = make((_, text) async {
      final s = CallSignal.tryParse(text)!;
      sent.add(s);
      return s.type == 'invite' ? await gate.future : true;
    });
    final starting = c.start('peer', withVideo: false);
    await t.pump();
    await c.hangup();
    gate.complete(true);
    await starting;
    await t.pump(const Duration(seconds: 4));
    expect(c.phase, CallPhase.idle);
    expect(sent.map((s) => s.type), ['invite', 'hangup']);
    c.dispose();
    await t.pump();
  });
  testWidgets('receipt before initial send completes still suppresses retries',
      (t) async {
    final gate = Completer<bool>();
    var sends = 0;
    final c = make((_, __) async {
      sends++;
      return gate.future;
    });
    final starting = c.start('peer', withVideo: false);
    await t.pump();
    await c.onSignal('peer', CallSignal.now('ringing', c.cid!));
    gate.complete(true);
    await starting;
    await t.pump(const Duration(seconds: 5));
    expect(sends, 1);
    c.dispose();
    await t.pump();
  });
}
