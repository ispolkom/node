import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/foundation.dart';
import 'package:record/record.dart';

/// Запись голосового сообщения. Абстракция нужна, чтобы чат проверялся без микрофона.
abstract class VoiceBackend {
  Future<bool> hasPermission();
  Future<void> start(String path);

  /// Остановить и вернуть путь к записанному файлу (null — записи нет).
  Future<String?> stop();
  Future<void> dispose();
}

/// AAC в контейнере m4a, как в телефоне: файл играет и там, и здесь.
class RecordBackend implements VoiceBackend {
  AudioRecorder? _r;
  AudioRecorder get r => _r ??= AudioRecorder();

  @override
  Future<bool> hasPermission() => r.hasPermission();

  @override
  Future<void> start(String path) => r.start(
    const RecordConfig(
      encoder: AudioEncoder.aacLc,
      bitRate: 48000,
      sampleRate: 32000,
      numChannels: 1,
    ),
    path: path,
  );

  @override
  Future<String?> stop() => r.stop();

  @override
  Future<void> dispose() async => _r?.dispose();
}

/// Фабрика, которую подменяют в тестах.
VoiceBackend Function() voiceBackendFactory = RecordBackend.new;

/// Куда писать голосовое: временный файл с понятным именем (как «Голосовое_…» в телефоне).
Future<String> newVoicePath() async {
  final t = DateTime.now();
  String two(int n) => n.toString().padLeft(2, '0');
  final name =
      'Голосовое_${t.year}${two(t.month)}${two(t.day)}_${two(t.hour)}${two(t.minute)}${two(t.second)}.m4a';
  final dir = await Directory.systemTemp.createTemp('yandi_voice');
  return '${dir.path}/$name';
}

/// Подменяется в тестах (настоящая файловая система в виджет-тестах не нужна).
Future<String> Function() voicePathFactory = newVoicePath;

/// Затирает временный файл записи и убирает его папку.
Future<void> wipeVoice(File f) async {
  try {
    if (await f.exists()) {
      final n = await f.length();
      final h = await f.open(mode: FileMode.write);
      await h.writeFrom(Uint8List(n.clamp(0, 64 * 1024 * 1024)));
      await h.close();
      await f.delete();
    }
    final dir = f.parent;
    if (dir.path.contains('yandi_voice') && await dir.exists()) {
      await dir.delete(recursive: true);
    }
  } catch (_) {}
}

/// Ход записи: таймер, предел длины, остановка. Экран только показывает и отправляет результат.
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
    if (!await backend.hasPermission()) {
      return 'Микрофон недоступен: проверьте доступ и что установлены parecord и ffmpeg.';
    }
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

  /// Остановить запись; возвращает (путь, секунды). Файл нужно затереть после отправки или отмены.
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
