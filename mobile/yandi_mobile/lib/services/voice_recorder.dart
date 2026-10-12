import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:record/record.dart';

/// Запись голосового сообщения. Абстракция нужна, чтобы экран чата проверялся без микрофона.
abstract class VoiceBackend {
  Future<bool> hasPermission();
  Future<void> start(String path);
  /// Остановить и вернуть путь к записанному файлу (null — записи нет).
  Future<String?> stop();
  Future<void> dispose();
}

/// AAC в контейнере m4a: небольшой размер, хорошо играет на телефоне и на ПК.
class RecordBackend implements VoiceBackend {
  final AudioRecorder _r = AudioRecorder();

  @override
  Future<bool> hasPermission() => _r.hasPermission();

  @override
  Future<void> start(String path) =>
      _r.start(const RecordConfig(encoder: AudioEncoder.aacLc, bitRate: 48000, sampleRate: 32000, numChannels: 1), path: path);

  @override
  Future<String?> stop() => _r.stop();

  @override
  Future<void> dispose() => _r.dispose();
}

/// Фабрика, которую подменяют в тестах.
VoiceBackend Function() voiceBackendFactory = RecordBackend.new;

/// Ход записи голосового: таймер, предел длины, остановка. Экран только показывает и отправляет результат.
class VoiceRecording extends ChangeNotifier {
  final VoiceBackend backend;
  final Future<String> Function() newPath;
  final int maxSeconds;
  bool active = false;
  int seconds = 0;
  String? path;
  Timer? _timer;
  /// Достигнут предел длины: вызывающий должен завершить запись.
  VoidCallback? onLimit;
  VoiceRecording(this.backend, this.newPath, {this.maxSeconds = 600});

  /// null — запись идёт; иначе — причина, почему не началась.
  Future<String?> start() async {
    if (active) return null;
    if (!await backend.hasPermission()) return 'Нет доступа к микрофону: разрешите его в настройках телефона';
    final p = await newPath();
    try {
      await backend.start(p);
    } catch (e) {
      return 'Не удалось начать запись: $e';
    }
    path = p;
    active = true;
    seconds = 0;
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      seconds++;
      notifyListeners();
      if (seconds >= maxSeconds) onLimit?.call();
    });
    notifyListeners();
    return null;
  }

  /// Остановить запись; возвращает (путь, секунды). Путь нужно затереть после отправки или отмены.
  Future<(String?, int)> stop() async {
    if (!active) return (null, 0);
    _timer?.cancel();
    final secs = seconds;
    String? out;
    try {
      out = await backend.stop() ?? path;
    } catch (_) {
      out = path;
    }
    active = false;
    seconds = 0;
    path = null;
    notifyListeners();
    return (out, secs);
  }

  @override
  void dispose() {
    _timer?.cancel();
    backend.dispose();
    super.dispose();
  }
}
