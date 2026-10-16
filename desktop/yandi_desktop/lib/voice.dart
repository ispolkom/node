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

/// Каталог для временных открытых файлов: оперативная память (tmpfs сеанса пользователя), а не диск. Так открытая копия не попадает на
/// накопитель и исчезает при выходе из сеанса; без tmpfs — системный каталог (крайний случай).
Directory ramTempRoot() {
  for (final p in [Platform.environment['XDG_RUNTIME_DIR'], '/dev/shm']) {
    if (p != null && p.isNotEmpty && Directory(p).existsSync()) return Directory(p);
  }
  return Directory.systemTemp;
}

/// Куда писать голосовое: временный файл с понятным именем (как «Голосовое_…» в телефоне).
Future<String> newVoicePath() async {
  final t = DateTime.now();
  String two(int n) => n.toString().padLeft(2, '0');
  final name =
      'Голосовое_${t.year}${two(t.month)}${two(t.day)}_${two(t.hour)}${two(t.minute)}${two(t.second)}.m4a';
  await _sweepOldVoice();
  final dir = await ramTempRoot().createTemp('yandi_voice');
  return '${dir.path}/$name';
}

/// Затереть остатки прежних запусков (голосовые и открытые копии вложений): вызывается при старте оболочки.
Future<void> sweepTempFiles() => _sweepOldVoice();

bool _swept = false;

/// Остатки прежних запусков (аварийное завершение): каталоги `yandi_voice*` старше минуты затираются один раз за запуск.
Future<void> _sweepOldVoice() async {
  if (_swept) return;
  _swept = true;
  try {
    // и прежние остатки в системном tmp (старые версии писали туда), и открытые копии вложений
    for (final root in {ramTempRoot().path, Directory.systemTemp.path}) {
    await for (final e in Directory(root).list(followLinks: false)) {
      final base = e.path.split('/').last;
      if (e is! Directory || !(base.startsWith('yandi_voice') || base.startsWith('yandi_open'))) continue;
      final age = DateTime.now().difference((await e.stat()).modified);
      if (age < const Duration(minutes: 1)) continue;
      await for (final f in e.list(followLinks: false)) {
        if (f is File) await wipeVoice(f);
      }
      if (await e.exists()) await e.delete(recursive: true);
    }
    }
  } catch (_) {}
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
    if ((dir.path.contains('yandi_voice') || dir.path.contains('yandi_open')) && await dir.exists()) {
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
  bool _disposed = false;

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
    if (_disposed) {
      // экран закрылся, пока запись начиналась: сразу остановить и затереть, ничего не оставляя в tmp
      try { await backend.stop(); } catch (_) {}
      await wipeVoice(File(p));
      return null;
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
    _disposed = true;
    _timer?.cancel();
    // запись, не доведённая до отправки/отмены (вкладка закрыта, блокировка, обрыв), не остаётся открытым файлом в tmp
    final leftover = path;
    final wasActive = active;
    active = false;
    path = null;
    unawaited(() async {
      if (wasActive) {
        try { await backend.stop(); } catch (_) {}
      }
      try { await backend.dispose(); } catch (_) {}
      if (leftover != null) await wipeVoice(File(leftover));
    }());
    super.dispose();
  }
}
