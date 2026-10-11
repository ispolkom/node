import 'dart:collection';
import 'dart:io';
import 'dart:typed_data';
import 'vault_service.dart';

/// Расшифрованные в памяти небольшие файлы (картинки для списка и чата, аватарки): чтобы не расшифровывать один и тот же файл при
/// каждой перерисовке. Только в оперативной памяти, ограничена по объёму, очищается при блокировке.
class VaultBytesCache {
  static const _limit = 24 * 1024 * 1024;
  static const _maxOne = 8 * 1024 * 1024;
  static final _items = LinkedHashMap<String, Uint8List>();
  static final _loading = <String, Future<Uint8List?>>{};
  static int _size = 0;

  static Future<Uint8List?> load(String path) {
    final hit = _items.remove(path);
    if (hit != null) {
      _items[path] = hit; // «недавно использован»
      return Future.value(hit);
    }
    return _loading[path] ??= _read(path).whenComplete(() => _loading.remove(path));
  }

  static Future<Uint8List?> _read(String path) async {
    try {
      final f = File(path);
      final v = await VaultService.instance.files();
      final meta = await v.readMeta(f);
      if (meta.size > _maxOne * 4) return null;   // совсем большие картинки в списках не показываем
      final bytes = await v.readRange(f, 0, meta.size);
      if (bytes.length <= _maxOne) _put(path, bytes);
      return bytes;
    } catch (_) {
      return null;
    }
  }

  static void _put(String path, Uint8List b) {
    _items[path] = b;
    _size += b.length;
    while (_size > _limit && _items.isNotEmpty) {
      final k = _items.keys.first;
      _size -= _items.remove(k)!.length;
    }
  }

  static void forget(String path) {
    final b = _items.remove(path);
    if (b != null) _size -= b.length;
  }

  static void clear() {
    _items.clear();
    _loading.clear();
    _size = 0;
  }
}
