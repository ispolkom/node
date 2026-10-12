import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/calls.dart';
import 'package:yandi_desktop/domain/call_coordinator.dart';
import 'package:yandi_desktop/domain/call_signaling.dart';
import 'widget_test.dart' show FakeApi, start, click, stop;

const peer = '0123456789abcdef';
const phonePeer =
    '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
const ref = CallRef(peer, 'call0001', video: true);

class Signals implements CallSignaling {
  final controller = StreamController<CallSignal>.broadcast();
  final commands = <String>[];
  List<CallRef> incoming = [];
  Completer<CallRef>? lateStart;
  bool closed = false;
  @override
  Stream<CallSignal> get signals => controller.stream;
  @override
  Future<CallRef> start(String peer, bool video, bool phone) async {
    commands.add('start');
    return lateStart == null
        ? CallRef(peer, 'call0001', video: video, phone: phone)
        : lateStart!.future;
  }

  @override
  Future<void> watch(CallRef c) async {
    commands.add('watch');
  }

  @override
  Future<List<CallRef>> poll() async {
    final r = incoming;
    incoming = [];
    return r;
  }

  @override
  Future<void> control(CallRef c, String t) async {
    commands.add('$t:${c.id}');
  }

  @override
  Future<void> send(CallRef c, Map<String, dynamic> d) async {
    commands.add('signal:${d['type']}');
  }

  @override
  Future<Map<String, dynamic>?> turn() async => null;
  @override
  Future<void> log(CallRef c, String text) async {
    commands.add('log:${c.id}');
  }

  @override
  void release() {
    commands.add('release');
  }

  @override
  void close() {
    closed = true;
    controller.close();
  }

  void signal(String type, {String id = 'call0001', String sender = peer}) =>
      controller.add(CallSignal(sender, {'type': type, 'call_id': id}));
}

class Media implements CallMedia {
  final controller = StreamController<MediaEvent>.broadcast();
  final received = <String>[];
  Completer<void>? acquisition;
  bool closed = false, muted = false, cameraOn = true;
  @override
  Stream<MediaEvent> get events => controller.stream;
  @override
  Future<void> open(CallRef c, Map<String, dynamic>? turn) async {
    if (acquisition != null) await acquisition!.future;
  }

  @override
  Future<void> offer() async {
    received.add('local-offer');
  }

  @override
  Future<void> receive(Map<String, dynamic> s) async {
    received.add('${s['type']}');
  }

  @override
  Future<void> mute(bool v) async {
    muted = v;
  }

  @override
  Future<void> camera(bool v) async {
    cameraOn = v;
  }

  @override
  void close() {
    closed = true;
  }
}

class PhoneApi extends FakeApi {
  List<Map<String, dynamic>> events = [];
  int last = 0;
  final texts = <String>[];
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    calls.add(path);
    if (path.endsWith('incoming-call')) return {'incoming': null};
    if (path.startsWith('/api/mobile/call/events'))
      return {'last': last, 'events': events};
    if (path == '/api/mobile/call/signal') {
      texts.add(body!['text']);
      return {'status': 'ok'};
    }
    return super.request(path, body: body, method: method);
  }
}

Future<void> drain() async {
  for (var i = 0; i < 12; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

Map<String, dynamic> phoneEvent(
  int seq,
  String t, {
  String cid = 'call0001',
  int? ts,
}) => {
  'seq': seq,
  'from': phonePeer,
  'name': 'Phone',
  'text':
      '\u0001yandi-call:${jsonEncode({'v': 1, 't': t, 'cid': cid, 'video': false, 'ts': ts ?? DateTime.now().millisecondsSinceEpoch})}',
};
void main() {
  testWidgets(
    'one coordinator outgoing video, matching control, ICE order and mute/camera',
    (t) async {
      final s = Signals(), m = Media();
      final c = CallCoordinator(
        signaling: s,
        authorize: () async => true,
        mediaFactory: () => m,
      );
      await c.start(peer, video: true);
      expect(c.phase, CallPhase.outgoingRinging);
      s.signal('call-accept', id: 'wrong000');
      await t.pump();
      expect(c.phase, CallPhase.outgoingRinging);
      s.signal('call-accept', sender: 'ffffffffffffffff');
      await t.pump();
      expect(c.phase, CallPhase.outgoingRinging);
      s.signal('call-accept');
      await t.pump();
      expect(c.phase, CallPhase.connecting);
      s.signal('video-ice');
      await t.pump();
      expect(m.received, ['local-offer']);
      s.signal('video-answer');
      await t.pump();
      expect(m.received, ['local-offer', 'video-answer', 'video-ice']);
      m.controller.add(const MediaEvent('connected'));
      await t.pump();
      expect(c.phase, CallPhase.active);
      await t.pump(const Duration(seconds: 3));
      expect(c.seconds, 3);
      await c.mute();
      await c.camera();
      expect(m.muted, true);
      expect(m.cameraOn, false);
      await c.end();
      expect(c.phase, CallPhase.ended);
      expect(m.closed, true);
      await t.pump(const Duration(seconds: 2));
      expect(c.phase, CallPhase.idle);
      expect(c.call, isNull);
      c.dispose();
    },
  );
  testWidgets('incoming accept reject and busy across voice/video/phone', (
    t,
  ) async {
    final s = Signals(), m = Media();
    final c = CallCoordinator(
      signaling: s,
      authorize: () async => true,
      mediaFactory: () => m,
    );
    await c.receiveCall(const CallRef(peer, 'call0001', incoming: true));
    s.incoming = [
      const CallRef(peer, 'call0002', incoming: true, video: true, phone: true),
    ];
    await c.poll();
    expect(s.commands, contains('busy:call0002'));
    await c.end(reject: true);
    expect(s.commands, contains('reject:call0001'));
    await t.pump(const Duration(seconds: 2));
    await c.receiveCall(
      const CallRef(peer, 'call0003', incoming: true, video: true),
    );
    await c.accept();
    expect(c.phase, CallPhase.connecting);
    expect(s.commands, contains('accept:call0003'));
    c.dispose();
    expect(m.closed, true);
    expect(s.closed, true);
  });
  testWidgets('ring 45s, connect 30s, disconnect grace 10s and recovery', (
    t,
  ) async {
    final s = Signals();
    Media m = Media();
    final c = CallCoordinator(
      signaling: s,
      authorize: () async => true,
      mediaFactory: () => m,
    );
    await c.start(peer);
    await t.pump(const Duration(seconds: 44));
    expect(c.phase, CallPhase.outgoingRinging);
    await t.pump(const Duration(seconds: 1));
    await t.pump();
    expect(c.phase, CallPhase.error);
    await t.pump(const Duration(seconds: 2));
    await c.start(peer);
    s.signal('call-accept');
    await t.pump();
    await t.pump(const Duration(seconds: 30));
    await t.pump();
    expect(c.phase, CallPhase.error);
    await t.pump(const Duration(seconds: 2));
    m = Media();
    await c.start(peer);
    s.signal('call-accept');
    await t.pump();
    m.controller.add(const MediaEvent('connected'));
    await t.pump();
    m.controller.add(const MediaEvent('disconnected'));
    await t.pump();
    await t.pump(const Duration(seconds: 9));
    expect(c.phase, CallPhase.active);
    m.controller.add(const MediaEvent('connected'));
    await t.pump();
    await t.pump(const Duration(seconds: 2));
    expect(c.phase, CallPhase.active);
    m.controller.add(const MediaEvent('disconnected'));
    await t.pump();
    await t.pump(const Duration(seconds: 10));
    await t.pump();
    expect(c.phase, CallPhase.error);
    c.dispose();
  });
  testWidgets(
    'late media acquisition and late call start cannot revive disposed owner',
    (t) async {
      final s = Signals(), m = Media()..acquisition = Completer();
      final c = CallCoordinator(
        signaling: s,
        authorize: () async => true,
        mediaFactory: () => m,
      );
      await c.start(peer);
      s.signal('call-accept');
      await t.pump();
      c.dispose();
      m.acquisition!.complete();
      await t.pump();
      expect(m.closed, true);
      expect(m.received, isEmpty);
      final s2 = Signals()..lateStart = Completer();
      final c2 = CallCoordinator(signaling: s2, authorize: () async => true);
      final pending = c2.start(peer);
      await t.pump();
      c2.dispose();
      s2.lateStart!.complete(ref);
      await pending;
      expect(s2.commands, contains('hangup:call0001'));
    },
  );
  testWidgets(
    'default unavailable media reports error and hangs up, never active',
    (t) async {
      final s = Signals();
      final c = CallCoordinator(signaling: s, authorize: () async => true);
      await c.start(peer);
      s.signal('call-accept');
      await t.pump();
      expect(c.phase, CallPhase.error);
      expect(c.error, contains('WebRTC'));
      expect(s.commands, contains('hangup:call0001'));
      c.dispose();
    },
  );
  testWidgets(
    'remote hangup interrupts pending media acquisition immediately',
    (t) async {
      final s = Signals(), m = Media()..acquisition = Completer();
      final c = CallCoordinator(
        signaling: s,
        authorize: () async => true,
        mediaFactory: () => m,
      );
      await c.start(peer);
      s.signal('call-accept');
      await t.pump();
      expect(c.phase, CallPhase.connecting);
      s.signal('hangup');
      await t.pump();
      expect(c.phase, CallPhase.ended);
      expect(m.closed, true);
      m.acquisition!.complete();
      await t.pump();
      expect(c.phase, CallPhase.ended);
      expect(m.received, isEmpty);
      c.dispose();
    },
  );
  testWidgets(
    'phone controls require matching peer and cid; active voice mute',
    (t) async {
      final s = Signals(), m = Media();
      final c = CallCoordinator(
        signaling: s,
        authorize: () async => true,
        mediaFactory: () => m,
      );
      await c.start(peer, phone: true);
      s.controller.add(
        const CallSignal(peer, {
          'v': 1,
          't': 'accept',
          'cid': 'wrong000',
          'video': false,
        }),
      );
      await t.pump();
      expect(c.phase, CallPhase.outgoingRinging);
      s.controller.add(
        const CallSignal(peer, {
          'v': 1,
          't': 'accept',
          'cid': 'call0001',
          'video': false,
        }),
      );
      await t.pump();
      expect(c.phase, CallPhase.connecting);
      m.controller.add(const MediaEvent('connected'));
      await t.pump();
      await c.mute();
      expect(m.muted, true);
      await c.camera();
      expect(m.cameraOn, true);
      s.controller.add(
        const CallSignal(peer, {
          'v': 1,
          't': 'hangup',
          'cid': 'call0001',
          'video': false,
        }),
      );
      await t.pump();
      expect(c.phase, CallPhase.ended);
      expect(s.commands, contains('log:call0001'));
      c.dispose();
    },
  );
  test(
    'phone events baseline, sequence, timestamp and outgoing envelope',
    () async {
      final api = PhoneApi()
        ..last = 1
        ..events = [phoneEvent(1, 'invite')];
      final s = DaemonCallSignaling(api);
      expect(await s.poll(), isEmpty);
      api.last = 3;
      api.events = [phoneEvent(2, 'invite'), phoneEvent(3, 'invite', ts: 0)];
      final incoming = await s.poll();
      expect(incoming.single.id, 'call0001');
      expect(await s.poll(), isEmpty);
      final c = await s.start(phonePeer, false, true);
      expect(c.id.length, 32);
      final wire = jsonDecode(api.texts.single.substring(12));
      expect(wire['t'], 'invite');
      expect(wire['video'], false);
      expect(wire['cid'], c.id);
      expect(api.calls, contains('/api/mobile/call/events?after=1'));
      s.close();
    },
  );
  testWidgets(
    'call screen incoming controls, stub error, navigation and lock clear input',
    (t) async {
      final api = PhoneApi(), s = Signals();
      await t.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: CallsFeature(
                api: api,
                isActive: () => true,
                onLocked: () {},
                onFailure: (_) {},
                signaling: s,
              ),
            ),
          ),
        ),
      );
      await t.pumpAndSettle();
      s.incoming = [const CallRef(peer, 'call0001', incoming: true)];
      await t.pump(const Duration(seconds: 1));
      await t.pumpAndSettle();
      expect(find.text('Входящий звонок'), findsOneWidget);
      await click(t, 'Принять');
      expect(find.textContaining('Медиа недоступно'), findsOneWidget);
      await stop(t);
      expect(s.closed, true);
      await start(t, api);
      await click(t, 'Войти');
      await click(t, 'Звонки');
      await t.enterText(find.byType(TextField).first, peer);
      await click(t, 'Панель');
      await click(t, 'Звонки');
      expect(
        (t.widget<TextField>(find.byType(TextField).first)).controller!.text,
        isEmpty,
      );
      await t.enterText(find.byType(TextField).first, peer);
      api.configured = true;
      api.unlocked = false;
      await t.pump(const Duration(seconds: 1));
      await t.pumpAndSettle();
      expect(find.text('Ожидание звонка'), findsNothing);
      await stop(t);
    },
  );
  testWidgets('call screen small window and preflight denial', (t) async {
    await t.binding.setSurfaceSize(const Size(400, 500));
    addTearDown(() => t.binding.setSurfaceSize(null));
    final api = PhoneApi();
    await start(t, api);
    await click(t, 'Войти');
    await click(t, 'Звонки');
    expect(t.takeException(), isNull);
    await t.enterText(find.byType(TextField).first, peer);
    api.configured = true;
    api.unlocked = false;
    await click(t, 'Позвонить');
    expect(api.calls.where((p) => p.endsWith('call/start')), isEmpty);
    await stop(t);
  });
}
