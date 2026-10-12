import 'dart:io';
import 'package:flutter/services.dart';
import 'loopback_media.dart';
import 'vault_service.dart';

/// Кадр видео для превью в чате. Видео расшифровывается кусками и отдаётся системному извлекателю кадров по 127.0.0.1:
/// ни файла, ни открытой копии на диске нет. Готовые кадры хранятся только в памяти (до закрытия приложения).
class VideoFrames {
  static const _ch = MethodChannel('com.yandi.yandi_mobile/files');
  static final _cache = <String, Uint8List?>{};
  static final _pending = <String, Future<Uint8List?>>{};
  static const _limit = 60;

  /// Подмена в тестах: (путь, размер, mime) → JPEG-байты.
  static Future<Uint8List?> Function(String path, int size, String mime)? grabber;

  static Uint8List? cached(String path) => _cache[path];

  static Future<Uint8List?> of(String path, int size, String mime) {
    if (_cache.containsKey(path)) return Future.value(_cache[path]);
    return _pending[path] ??= _grab(path, size, mime).then((v) {
      if (_cache.length >= _limit) _cache.remove(_cache.keys.first);
      _cache[path] = v;
      _pending.remove(path);
      return v;
    });
  }

  static Future<Uint8List?> _grab(String path, int size, String mime) async {
    final g = grabber;
    if (g != null) return g(path, size, mime);
    LoopbackMedia? m;
    try {
      final vault = await VaultService.instance.files();
      final file = File(path);
      m = LoopbackMedia.reader(size, mime, (s, n) => vault.readRange(file, s, n));
      final url = await m.start();
      final data = await _ch.invokeMethod<Uint8List>('videoFrame', {'url': url.toString(), 'ms': 1000, 'maxWidth': 480});
      return data;
    } catch (_) {
      return null;
    } finally {
      await m?.stop();
    }
  }

  static void clear() {
    _cache.clear();
    _pending.clear();
  }
}
