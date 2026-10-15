import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/voice.dart';

class _WritingVoice implements VoiceBackend {
  String? written;
  @override
  Future<bool> hasPermission() async => true;
  @override
  Future<void> start(String path) async {
    written = path;
    await File(path).writeAsBytes(List.filled(1000, 7));
  }

  @override
  Future<String?> stop() async => written;
  @override
  Future<void> dispose() async {}
}

void main() {
  test('закрытие экрана во время записи не оставляет файл в tmp', () async {
    final backend = _WritingVoice();
    final rec = VoiceRecording(backend, newVoicePath);
    expect(await rec.start(), isNull);
    final f = File(backend.written!);
    expect(await f.exists(), isTrue);
    rec.dispose();
    for (var i = 0; i < 50 && await f.exists(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(await f.exists(), isFalse, reason: 'временная запись затёрта и удалена');
    expect(await f.parent.exists(), isFalse, reason: 'каталог убран');
  });
}
