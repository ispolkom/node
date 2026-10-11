import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:cryptography/cryptography.dart';
import 'package:flutter/foundation.dart';
import '../models/avatar_msg.dart';

/// Шифрование картинок ключом хранилища (AES-256-GCM, хэш картинки входит в проверку).
class AvatarSealer {
  AvatarSealer(this._key);
  final Future<List<int>> Function() _key;
  static final _aes = AesGcm.with256bits();

  Future<Uint8List> seal(Uint8List png, String hash) async {
    final box = await _aes.encrypt(png, secretKey: SecretKey(await _key()), aad: utf8.encode('yandi-avatar:v1:$hash'));
    return Uint8List.fromList(box.concatenation());
  }

  Future<Uint8List?> open(Uint8List blob, String hash) async {
    try {
      final box = SecretBox.fromConcatenation(blob, nonceLength: 12, macLength: 16);
      return Uint8List.fromList(await _aes.decrypt(box, secretKey: SecretKey(await _key()), aad: utf8.encode('yandi-avatar:v1:$hash')));
    } catch (_) {
      return null;
    }
  }
}

/// Аватарки: своя и контактов. Файлы лежат в изолированной папке приложения под именем `<хэш>.yav` (зашифрованы; одинаковые картинки — один файл),
/// соответствие «контакт → хэш» хранится в защищённом хранилище. Ключ в соответствии — пара (номер контакта, хэш): у десяти людей с одной и
/// той же картинкой будет десять записей и один файл, и они не перепутаются.
///
/// Кому слать и от кого принимать — решает приложение (только сохранённые контакты, не заблокированные, не сам узел); здесь только протокол.
class AvatarService extends ChangeNotifier {
  final Directory dir;
  final Future<String?> Function() _read;
  final Future<void> Function(String) _write;
  /// Затирание поверх байтов перед удалением (на телефоне — нативное); без него картинка просто удаляется.
  final Future<bool> Function(String path)? wipe;
  /// Шифрование файлов картинок; расшифрованные картинки держатся только в памяти, пока приложение открыто.
  final AvatarSealer sealer;
  AvatarService({required this.dir, required Future<String?> Function() read, required Future<void> Function(String) write, required this.sealer, this.wipe})
      : _read = read,
        _write = write;
  final Map<String, Uint8List> _bytes = {};

  String? _mine;
  final Map<String, String> _peers = {};
  /// Запрошенные картинки: принимаем только то, что просили (иначе контакт мог бы забивать нам память чем угодно).
  final Map<String, DateTime> _pending = {};
  static const _pendingTtl = Duration(minutes: 5);

  String? get myHash => _mine;
  Uint8List? get myBytes => _mine == null ? null : _bytes[_mine!];
  String? hashForPeer(String peerId) => _peers[peerId];
  Uint8List? bytesForPeer(String peerId) {
    final h = _peers[peerId];
    return h == null ? null : _bytes[h];
  }

  File _file(String hash) => File('${dir.path}/$hash.yav');
  File _legacyFile(String hash) => File('${dir.path}/$hash.png');

  /// Файл картинки в память: зашифрованный открывается; старый открытый `.png` шифруется, оригинал затирается.
  Future<void> _loadFile(String hash) async {
    final f = _file(hash);
    if (await f.exists()) {
      final png = await sealer.open(await f.readAsBytes(), hash);
      if (png != null && AvatarMsg.hashOf(png) == hash) _bytes[hash] = png;
      return;
    }
    final old = _legacyFile(hash);
    if (await old.exists()) {
      final png = await old.readAsBytes();
      if (AvatarMsg.validPng(png) && AvatarMsg.hashOf(png) == hash) {
        await _store(png, hash);
        final w = wipe;
        if (w == null) {
          if (await old.exists()) await old.delete();
        } else {
          await w(old.path);   // отказ затирания: открытая картинка остаётся и будет дочищена при следующем входе, а не удаляется молча
        }
      }
    }
  }

  Future<void> load() async {
    try {
      final raw = await _read();
      if (raw != null) {
        final m = jsonDecode(raw) as Map<String, dynamic>;
        final me = m['me'] as String?;
        _mine = (me != null && AvatarMsg.validHash(me)) ? me : null;
        _peers.clear();
        ((m['peers'] as Map?) ?? {}).forEach((k, v) { if (v is String && AvatarMsg.validHash(v)) _peers[k as String] = v; });
        for (final h in {if (_mine != null) _mine!, ..._peers.values}) {
          await _loadFile(h);
        }
        // индекс прочитан: дочищаем то, что в прошлый раз не удалось затереть (при ошибке чтения неизвестные файлы не трогаем)
        await _collectGarbage();
      }
    } catch (_) {}
    notifyListeners();
  }

  Future<void> _save() async {
    await _write(jsonEncode({'me': _mine, 'peers': _peers}));
    await _collectGarbage();
    notifyListeners();
  }

  /// Файлы, на которые никто не ссылается, удаляются (отвязанный контакт, замена своей картинки).
  Future<void> _collectGarbage() async {
    try {
      if (!await dir.exists()) return;
      final used = {if (_mine != null) _mine!, ..._peers.values};
      _bytes.removeWhere((h, _) => !used.contains(h));
      await for (final e in dir.list(followLinks: false)) {
        if (e is! File) continue;
        final name = e.uri.pathSegments.last;
        if (!name.endsWith('.yav') || !used.contains(name.substring(0, name.length - 4))) {
          final w = wipe;
          if (w == null) {
            await e.delete();
          } else {
            await w(e.path);   // не вышло затереть — файл остаётся и снова попадёт в уборку при следующем запуске
          }
        }
      }
    } catch (_) {}
  }

  Future<void> _store(Uint8List png, String hash) async {
    if (!await dir.exists()) await dir.create(recursive: true);
    final f = _file(hash);
    if (!await f.exists()) {
      final tmp = File('${f.path}.tmp');
      await tmp.writeAsBytes(await sealer.seal(png, hash), flush: true);
      await tmp.rename(f.path);
    }
    _bytes[hash] = png;
  }

  // ── своя ──

  /// Установить свою картинку (уже подготовленный PNG). Возвращает её хэш.
  Future<String> setMine(Uint8List png) async {
    if (!AvatarMsg.validPng(png)) throw StateError('Некорректная картинка');
    final h = AvatarMsg.hashOf(png);
    await _store(png, h);
    _mine = h;
    await _save();
    return h;
  }

  Future<void> clearMine() async {
    _mine = null;
    await _save();
  }

  // ── протокол ──

  /// Контакт сообщил хэш своей картинки. [send] отправляет ему текст (запрос картинки).
  Future<void> onAnnounce(String from, String hash, Future<void> Function(String text) send) async {
    if (hash.isEmpty) {
      if (_peers.remove(from) != null) await _save();   // убрал картинку
      return;
    }
    if (_peers[from] == hash && _bytes.containsKey(hash)) return;   // уже такая
    if (_bytes.containsKey(hash)) {   // такая картинка у нас уже есть (у другого контакта): просто привязываем, ничего не качаем
      _peers[from] = hash;
      await _save();
      return;
    }
    _pending['$from|$hash'] = DateTime.now();
    await send(AvatarMsg.request(hash));
  }

  /// Контакт просит нашу картинку: отдаём только если просит именно ту, что у нас сейчас.
  Future<void> onRequest(String from, String hash, Future<void> Function(String text) send) async {
    final mine = _mine;
    if (mine == null || hash != mine) return;
    final png = _bytes[mine];
    if (png == null) return;
    if (AvatarMsg.hashOf(png) != mine || !AvatarMsg.validPng(png)) return;
    await send(AvatarMsg.image(mine, png));
  }

  /// Пришла картинка. Принимается только запрошенная, с совпавшим хэшем, нужного размера и формата (и читаемая декодером, если он задан).
  Future<bool> onImage(String from, String hash, Uint8List png, {Future<bool> Function(Uint8List)? decodes}) async {
    final key = '$from|$hash';
    final asked = _pending.remove(key);
    if (asked == null || DateTime.now().difference(asked) > _pendingTtl) return false;
    if (!AvatarMsg.validPng(png) || AvatarMsg.hashOf(png) != hash) return false;
    if (decodes != null && !await decodes(png)) return false;
    await _store(png, hash);
    _peers[from] = hash;
    await _save();
    return true;
  }

  /// Блокировка: расшифрованные картинки из памяти стираются (сами буферы, а не только ссылки).
  void wipeMemory() {
    for (final b in _bytes.values) {
      b.fillRange(0, b.length, 0);
    }
    _bytes.clear();
  }

  Future<void> forgetPeer(String peerId) async {
    if (_peers.remove(peerId) != null) await _save();
  }
}
