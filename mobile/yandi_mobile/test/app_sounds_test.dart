import 'dart:async';
import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/call_signal.dart';
import 'package:yandi_mobile/services/app_sounds.dart';
import 'package:yandi_mobile/services/call_service.dart';

class TonePlayer extends AudioPlayer {
  static int next = 0;
  TonePlayer() : super(playerId: 'test-tone-${next++}') {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(
      MethodChannel('xyz.luan/audioplayers/events/$playerId'), (call) async => null);
  }
  final played = <String>[];
  AudioContext? context;
  Completer<void>? contextGate;
  int stops = 0;
  @override Future<void> setReleaseMode(ReleaseMode mode) async {}
  @override Future<void> setAudioContext(AudioContext value) async {
    context = value;
    await contextGate?.future;
  }
  @override Future<void> play(Source source, {double? volume, double? balance, AudioContext? ctx, Duration? position, PlayerMode? mode}) async {
    played.add((source as AssetSource).path);
  }
  @override Future<void> stop() async { stops++; }
}

Future<void> drain() async {
  for (var i = 0; i < 12; i++) { await Future<void>.delayed(Duration.zero); }
}

CallService service(TonePlayer player, Map<String, bool> enabled) => CallService(
  ringPlayer: player, soundEnabled: (kind) => enabled[kind] ?? true,
  sendSignal: (_, __) async => true, fetchTurn: () async => null,
  nodeHost: () => null, nameOf: (peer) => peer, showScreen: () {},
  onIncomingNotify: (_, __, ___) {}, onIncomingCancel: () {}, onLog: (_, __, ___) {},
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() {
    for (final name in ['xyz.luan/audioplayers', 'xyz.luan/audioplayers.global', 'xyz.luan/audioplayers.global/events', 'com.cloudwebrtc.webrtc/method']) {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(MethodChannel(name), (call) async => null);
    }
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMessageHandler(
      'dev.flutter.pigeon.wakelock_plus_platform_interface.WakelockPlusApi.toggle',
      (_) async => const StandardMessageCodec().encodeMessage(<Object?>[]));
  });

  test('all five mapped MP3s are included in the mobile asset bundle', () async {
    expect(AppSounds.assets, {'messages': 'icq.mp3', 'groups': 'icq-online.mp3', 'calls': 'calling.mp3', 'videoCalls': 'sqype.mp3', 'dialing': 'call_out.mp3'});
    for (final asset in AppSounds.assets.values) {
      expect((await rootBundle.load('assets/sounds/$asset')).lengthInBytes, greaterThan(1000));
    }
    expect(AppSounds.context('messages').android.usageType, AndroidUsageType.notification);
    expect(AppSounds.context('calls').android.usageType, AndroidUsageType.notificationRingtone);
    expect(AppSounds.context('videoCalls').android.usageType, AndroidUsageType.notificationRingtone);
  });

  for (final video in [false, true]) {
    test('incoming ${video ? 'video' : 'audio'} uses its own tone and reacts to its category switch', () async {
      final player = TonePlayer(), enabled = <String, bool>{};
      final svc = service(player, enabled);
      await svc.onSignal('peer', CallSignal.now('invite', 'call0001', video: video));
      await drain();
      expect(player.played.last, 'sounds/${video ? 'sqype.mp3' : 'calling.mp3'}');
      enabled[video ? 'videoCalls' : 'calls'] = false;
      svc.refreshRingtone();
      await drain();
      expect(player.stops, greaterThan(0));
      enabled[video ? 'videoCalls' : 'calls'] = true;
      svc.refreshRingtone();
      await drain();
      expect(player.played.length, 2);
      svc.dispose();
      await drain();
    });
  }

  test('cancel during audio setup cannot start a stale outgoing dial tone', () async {
    final player = TonePlayer()..contextGate = Completer<void>();
    final svc = service(player, {});
    await svc.start('peer', withVideo: false);
    await drain();
    await svc.hangup();
    player.contextGate!.complete();
    await drain();
    expect(player.played, isEmpty);
    expect(player.stops, greaterThan(0));
    svc.dispose();
    await drain();
  });
}
