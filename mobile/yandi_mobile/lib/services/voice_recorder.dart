import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart';
import 'package:record/record.dart';
import '../l10n_core.dart';

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
  bool _disposed = false;
  bool _starting = false;   // запуск идёт: второй запуск не допускается, пока первый не завершится или не будет отменён
  int _gen = 0;   // поколение запуска: stop/dispose во время запуска отменяют его
  /// Достигнут предел длины: вызывающий должен завершить запись.
  VoidCallback? onLimit;
  VoiceRecording(this.backend, this.newPath, {this.maxSeconds = 600});

  /// null — запись идёт; иначе — причина, почему не началась.
  Future<String?> start() async {
    if (active || _disposed) return null;
    if (_starting) return tr('Микрофон ещё запускается');   // двойное нажатие не запускает второй запуск поверх первого
    _starting = true;
    try {
      return await _startOnce();
    } finally {
      _starting = false;
    }
  }

  Future<String?> _startOnce() async {
    final gen = ++_gen;
    bool stale() => _disposed || gen != _gen;
    if (!await backend.hasPermission()) return tr('Нет доступа к микрофону: разрешите его в настройках телефона');
    if (stale()) return tr('Запись отменена');
    final p = await newPath();
    if (stale()) return tr('Запись отменена');
    try {
      await backend.start(p);
    } catch (e) {
      return trf('Не удалось начать запись: {e}', {'e': e});
    }
    if (stale()) {
      // чат закрыли (или запись отменили), пока запускался микрофон: остановить и стереть уже созданный файл
      try { await backend.stop(); } catch (_) {}
      await wipeLateFile(p);
      return tr('Запись отменена');
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
    _gen++;   // запуск, ещё не закончившийся, отменяется
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

  /// Стирание файла, запущенного слишком поздно; в тестах подменяется.
  Future<void> Function(String path) wipeLateFile = _defaultWipe;
  static Future<void> _defaultWipe(String p) async {
    try {
      final f = File(p);
      if (await f.exists()) {
        final n = await f.length();
        final h = await f.open(mode: FileMode.write);
        try { await h.writeFrom(List.filled(n < 1 << 20 ? n : 1 << 20, 0)); await h.flush(); } finally { await h.close(); }
        await f.delete();
      }
    } catch (_) {}
  }

  @override
  void dispose() {
    _disposed = true;
    _gen++;
    _timer?.cancel();
    backend.dispose();
    super.dispose();
  }
}
