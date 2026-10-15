import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/call_signal.dart';
import 'package:yandi_mobile/services/call_service.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  // плагин проигрывателя в тестах без платформы: на любой вызов — пустой успех
  for (final ch in ['xyz.luan/audioplayers', 'xyz.luan/audioplayers.global', 'com.cloudwebrtc.webrtc/method']) {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MethodChannel(ch), (call) async => null);
  }

  test('встречный invite с video=true не превращает исходящий аудиозвонок в видео', () async {
    final sent = <String>[];
    final svc = CallService(
      sendSignal: (peer, text) async { sent.add(text); return true; },
      fetchTurn: () async => null,
      nodeHost: () => null,
      nameOf: (p) => p,
      showScreen: () {},
      onIncomingNotify: (a, b, c) {},
      onIncomingCancel: () {},
      onLog: (a, b, c) {},
    );
    await svc.start('peer', withVideo: false);
    expect(svc.video, isFalse);
    // встречный invite с меньшим номером и видео
    await svc.onSignal('peer', CallSignal.now('invite', '0000000000000000', video: true));
    expect(svc.video, isFalse, reason: 'удалённое поле не включает камеру');
  });
}
