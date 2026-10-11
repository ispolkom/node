import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter/foundation.dart';
import '../models/avatar_msg.dart';

/// Аватарки: своя и контактов. Файлы лежат в изолированной папке приложения под именем `<хэш>.png` (одинаковые картинки — один файл),
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
  AvatarService({required this.dir, required Future<String?> Function() read, required Future<void> Function(String) write, this.wipe}) : _read = read, _write = write;

  String? _mine;
  final Map<String, String> _peers = {};
  /// Запрошенные картинки: принимаем только то, что просили (иначе контакт мог бы забивать нам память чем угодно).
  final Map<String, DateTime> _pending = {};
  static const _pendingTtl = Duration(minutes: 5);

  String? get myHash => _mine;
  File? get myFile => _mine == null ? null : _file(_mine!);
  String? hashForPeer(String peerId) => _peers[peerId];
  File? fileForPeer(String peerId) {
    final h = _peers[peerId];
    if (h == null) return null;
    final f = _file(h);
    return f.existsSync() ? f : null;
  }

  File _file(String hash) => File('${dir.path}/$hash.png');

  Future<void> load() async {
    try {
      final raw = await _read();
      if (raw != null) {
        final m = jsonDecode(raw) as Map<String, dynamic>;
        final me = m['me'] as String?;
        _mine = (me != null && AvatarMsg.validHash(me)) ? me : null;
        _peers.clear();
        ((m['peers'] as Map?) ?? {}).forEach((k, v) { if (v is String && AvatarMsg.validHash(v)) _peers[k as String] = v; });
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
      await for (final e in dir.list(followLinks: false)) {
        if (e is! File) continue;
        final name = e.uri.pathSegments.last;
        if (!name.endsWith('.png') || !used.contains(name.substring(0, name.length - 4))) {
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
      await tmp.writeAsBytes(png, flush: true);
      await tmp.rename(f.path);
    }
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
    if (_peers[from] == hash && _file(hash).existsSync()) return;   // уже такая
    if (_file(hash).existsSync()) {   // такая картинка у нас уже есть (у другого контакта): просто привязываем, ничего не качаем
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
    final f = _file(mine);
    if (!await f.exists()) return;
    final png = await f.readAsBytes();
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

  Future<void> forgetPeer(String peerId) async {
    if (_peers.remove(peerId) != null) await _save();
  }
}
