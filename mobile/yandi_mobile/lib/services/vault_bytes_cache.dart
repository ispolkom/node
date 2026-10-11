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
  /// Поколение: блокировка меняет его, и чтение, начатое до неё, не кладёт расшифрованное в кеш после неё.
  static int _gen = 0;

  static Future<Uint8List?> load(String path) {
    final hit = _items.remove(path);
    if (hit != null) {
      _items[path] = hit; // «недавно использован»
      return Future.value(hit);
    }
    // тело в фигурных скобках: стрелка вернула бы удалённый Future, и whenComplete стал бы ждать самого себя вечно
    return _loading[path] ??= _read(path).whenComplete(() {
      _loading.remove(path);
    });
  }

  static Future<Uint8List?> _read(String path) async {
    final g = _gen;
    try {
      final f = File(path);
      final v = await VaultService.instance.files();
      final meta = await v.readMeta(f);
      if (meta.size > _maxOne * 4) return null;   // совсем большие картинки в списках не показываем
      final bytes = await v.readRange(f, 0, meta.size);
      if (g != _gen) {
        bytes.fillRange(0, bytes.length, 0);   // пока читали, заблокировали
        return null;
      }
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
    _gen++;
    for (final b in _items.values) {
      b.fillRange(0, b.length, 0);   // сами буферы стираем, а не только ссылки на них
    }
    _items.clear();
    _loading.clear();
    _size = 0;
  }
}
