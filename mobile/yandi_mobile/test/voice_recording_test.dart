import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/voice_recorder.dart';

class FakeVoice implements VoiceBackend {
  bool permission = true, started = false, disposed = false, failStart = false;
  String? startedPath;
  @override
  Future<bool> hasPermission() async => permission;
  @override
  Future<void> start(String path) async {
    if (failStart) throw StateError('занят');
    started = true;
    startedPath = path;
  }
  @override
  Future<String?> stop() async {
    started = false;
    return startedPath;
  }
  @override
  Future<void> dispose() async {
    disposed = true;
  }
}

void main() {
  test('нет разрешения на микрофон — запись не начинается и сказано почему', () async {
    final b = FakeVoice()..permission = false;
    final r = VoiceRecording(b, () async => '/tmp/v.m4a');
    final err = await r.start();
    expect(err, contains('микрофон'));
    expect(r.active, false);
    expect(b.started, false);
  });

  test('ошибка запуска не оставляет запись активной', () async {
    final b = FakeVoice()..failStart = true;
    final r = VoiceRecording(b, () async => '/tmp/v.m4a');
    expect(await r.start(), contains('Не удалось'));
    expect(r.active, false);
  });

  test('таймер считает секунды, stop отдаёт путь и длительность', () {
    fakeAsync((fa) {
      final b = FakeVoice();
      final r = VoiceRecording(b, () async => '/tmp/v.m4a');
      r.start();
      fa.flushMicrotasks();
      expect(r.active, true);
      fa.elapse(const Duration(seconds: 3));
      expect(r.seconds, 3);
      (String?, int)? res;
      r.stop().then((v) => res = v);
      fa.flushMicrotasks();
      expect(res!.$1, '/tmp/v.m4a');
      expect(res!.$2, 3);
      expect(r.active, false);
      expect(r.seconds, 0);
      fa.elapse(const Duration(seconds: 5));
      expect(r.seconds, 0);
    });
  });

  test('предел длины вызывает onLimit', () {
    fakeAsync((fa) {
      final r = VoiceRecording(FakeVoice(), () async => '/tmp/v.m4a', maxSeconds: 2);
      var hit = 0;
      r.onLimit = () => hit++;
      r.start();
      fa.flushMicrotasks();
      fa.elapse(const Duration(seconds: 2));
      expect(hit, 1);
      r.dispose();
    });
  });
}
