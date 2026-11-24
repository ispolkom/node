import 'dart:async';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:yandi_mobile/models/call_signal.dart';
import 'package:yandi_mobile/services/call_service.dart';
import 'call_delivery_test.dart' show SilentPlayer;

class FakeStream implements MediaStream {
  bool released = false;
  @override
  List<MediaStreamTrack> getTracks() => [];
  @override
  Future<void> dispose() async { released = true; }
  @override
  dynamic noSuchMethod(Invocation i) => super.noSuchMethod(i);
}

class FakeRenderer implements RTCVideoRenderer {
  @override
  MediaStream? srcObject;
  @override
  Future<void> initialize() async {}
  @override
  Future<void> dispose() async {}
  @override
  dynamic noSuchMethod(Invocation i) => super.noSuchMethod(i);
}

class FakePeer implements RTCPeerConnection {
  bool closed = false;
  int answers = 0;
  Completer<void>? closeGate;
  @override
  void Function(RTCIceCandidate)? onIceCandidate;
  @override
  void Function(RTCTrackEvent)? onTrack;
  @override
  void Function(RTCPeerConnectionState)? onConnectionState;
  @override
  Future<void> close() async { await closeGate?.future; closed = true; }
  @override
  Future<void> dispose() async {}
  @override
  Future<void> setRemoteDescription(RTCSessionDescription sdp) async {}
  @override
  Future<void> setLocalDescription(RTCSessionDescription sdp) async {}
  @override
  Future<RTCSessionDescription> createAnswer([Map<String, dynamic>? constraints]) async {
    answers++;
    return RTCSessionDescription('answer', 'answer');
  }
  @override
  dynamic noSuchMethod(Invocation i) => super.noSuchMethod(i);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('FlutterWebRTC.Method'), (_) async => null);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMessageHandler('dev.flutter.pigeon.wakelock_plus_platform_interface.WakelockPlusApi.toggle',
            (_) async => const StandardMessageCodec().encodeMessage(<Object?>[]));
  });
  CallService make(FakePeer peer, Future<MediaStream> Function() capture,
      List<CallSignal> sent, {Future<RTCPeerConnection> Function(Map<String, dynamic>)? factory}) => CallService(
    sendSignal: (_, text) async { sent.add(CallSignal.tryParse(text)!); return true; },
    fetchTurn: () async => null, nodeHost: () => null, nameOf: (p) => p,
    showScreen: () {}, onIncomingNotify: (_, __, ___) {},
    onIncomingCancel: () {}, onLog: (_, __, ___) {},
    soundEnabled: (_) => false, ringPlayer: SilentPlayer(),
    createPeer: factory ?? ((_) async => peer), acquireMedia: (_) => capture(),
    createRenderer: FakeRenderer.new,
  );
  Future<void> incoming(CallService c, String id) =>
      c.onSignal('peer', CallSignal.now('invite', id));

  test('answer waits for capture and cancelled capture cannot affect next call', () async {
    final peer = FakePeer(), gate = Completer<MediaStream>(), sent = <CallSignal>[];
    final c = make(peer, () => gate.future, sent);
    await incoming(c, 'firstcall');
    final accepting = c.accept();
    await Future<void>.delayed(const Duration(milliseconds: 20));
    final offering = c.onSignal('peer', CallSignal.now('offer', 'firstcall', sdp: 'offer'));
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(peer.answers, 0);
    await c.hangup();
    await Future<void>.delayed(const Duration(seconds: 2));
    await incoming(c, 'secondcall');
    final stream = FakeStream();
    gate.complete(stream);
    await accepting;
    await offering;
    expect(stream.released, isTrue);
    expect(c.cid, 'secondcall');
    expect(c.phase, CallPhase.ringing);
    expect(sent.where((s) => s.type == 'answer'), isEmpty);
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 20));
  });

  test('old cleanup cannot close the next call and stale callbacks are ignored', () async {
    final first = FakePeer(), second = FakePeer(), sent = <CallSignal>[];
    var created = 0;
    final c = make(first, () async => FakeStream(), sent,
        factory: (_) async => created++ == 0 ? first : second);
    var opens = 0;
    // A second call starts only after the old native connection closes.
    await incoming(c, 'firstcall');
    await c.accept();
    final oldIce = first.onIceCandidate!, oldState = first.onConnectionState!;
    first.closeGate = Completer<void>();
    await c.hangup();
    await Future<void>.delayed(const Duration(seconds: 2));
    await incoming(c, 'secondcall');
    final accepting = c.accept().then((_) { opens++; });
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(opens, 0);
    oldIce(RTCIceCandidate('old-candidate', '0', 0));
    oldState(RTCPeerConnectionState.RTCPeerConnectionStateFailed);
    expect(c.phase, CallPhase.connecting);
    expect(sent.where((s) => s.type == 'ice'), isEmpty);
    first.closeGate!.complete();
    await accepting;
    expect(c.phase, CallPhase.connecting);
    expect(second.closed, isFalse);
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 20));
  });

  test('answer is sent only after capture is ready', () async {
    final peer = FakePeer(), gate = Completer<MediaStream>(), sent = <CallSignal>[];
    final c = make(peer, () => gate.future, sent);
    await incoming(c, 'firstcall');
    final accepting = c.accept();
    await Future<void>.delayed(const Duration(milliseconds: 20));
    final offering = c.onSignal('peer', CallSignal.now('offer', 'firstcall', sdp: 'offer'));
    await Future<void>.delayed(const Duration(milliseconds: 150));
    expect(peer.answers, 0);
    gate.complete(FakeStream());
    await accepting;
    await offering;
    expect(peer.answers, 1);
    expect(sent.where((s) => s.type == 'answer').length, 1);
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 20));
  });

  test('disposing an active capture releases media', () async {
    final peer = FakePeer(), stream = FakeStream(), sent = <CallSignal>[];
    final c = make(peer, () async => stream, sent);
    await incoming(c, 'firstcall');
    await c.accept();
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(stream.released, isTrue);
    expect(peer.closed, isTrue);
  });
}
