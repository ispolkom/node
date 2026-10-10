import 'dart:io';
import 'dart:math';
import 'file_transfer_service.dart';

/// Файл в изолированном хранилище приложения (папка внутри песочницы Android, недоступная другим приложениям).
class VaultFile {
  final String path;
  final String fileName;   // как на диске: <8 hex>_<имя>
  final String name;       // для человека
  final int size;
  final DateTime modified;
  final String mime;
  const VaultFile({required this.path, required this.fileName, required this.name, required this.size, required this.modified, required this.mime});

  bool get isImage => mime.startsWith('image/');
  bool get isAudio => mime.startsWith('audio/');
  bool get isVideo => mime.startsWith('video/');
  bool get isText => mime.startsWith('text/') || mime == 'application/json' || mime == 'application/xml';
}

/// Список, импорт и удаление файлов хранилища. Принятые файлы лежат здесь же (`yandi_downloads`), так что это тот же общий список.
class VaultFiles {
  final Directory dir;
  /// Затирание поверх байтов и удаление (на телефоне — нативное, `FileWipe`). Без него файл просто удаляется.
  final Future<bool> Function(String path)? wipe;
  VaultFiles(this.dir, {this.wipe});

  /// Фоновые затирания (чтобы тест и интерфейс могли дождаться конца).
  Future<void> _idle = Future.value();
  Future<void> get idle => _idle;

  static const _doomed = '.wipe';
  static final _prefix = RegExp(r'^[0-9a-f]{8}_');

  /// Имя без служебного префикса номера передачи.
  static String displayName(String fileName) => fileName.replaceFirst(_prefix, '');

  static String _randomPrefix() {
    final r = Random.secure();
    return List.generate(4, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  }

  Future<List<VaultFile>> list() async {
    if (!await dir.exists()) return [];
    final out = <VaultFile>[];
    await for (final e in dir.list(followLinks: false)) {
      if (e is! File) continue;
      final fileName = e.uri.pathSegments.last;
      if (fileName.endsWith('.part') || fileName.endsWith(_doomed)) continue;    // недокачанное и уже удаляемое не показываем
      final st = await e.stat();
      out.add(VaultFile(
        path: e.path, fileName: fileName, name: displayName(fileName), size: st.size, modified: st.modified,
        mime: FileTransferService.guessMime(fileName),
      ));
    }
    out.sort((a, b) => b.modified.compareTo(a.modified));
    return out;
  }

  /// Описание уже лежащего файла (например, полученного в чате) в том же виде, что и в списке.
  static Future<VaultFile> describe(String path) async {
    final f = File(path);
    final st = await f.stat();
    final fileName = f.uri.pathSegments.last;
    return VaultFile(path: path, fileName: fileName, name: displayName(fileName), size: st.size, modified: st.modified, mime: FileTransferService.guessMime(fileName));
  }

  /// Положить копию файла в хранилище (чтобы потом переслать оттуда). Исходный файл не трогаем.
  Future<VaultFile> importFile(String sourcePath, {String? name}) async {
    final base = FileTransferService.safeName(name ?? sourcePath.split(Platform.pathSeparator).last);
    if (!await dir.exists()) await dir.create(recursive: true);
    final target = File('${dir.path}/${_randomPrefix()}_$base');
    await File(sourcePath).copy(target.path);
    final st = await target.stat();
    return VaultFile(path: target.path, fileName: target.uri.pathSegments.last, name: base, size: st.size, modified: st.modified, mime: FileTransferService.guessMime(base));
  }

  /// Удаление сразу и насовсем, без корзины. Файл мгновенно пропадает из списка (переименование), а затирание поверх его байтов и
  /// удаление идут в фоне: на большом файле оно занимает время, и ждать его интерфейсу незачем. Недоделанное (закрыли приложение)
  /// дочищается при следующем открытии вкладки ([finishPendingWipes]).
  /// Честная оговорка: на флеш-памяти телефона затирание не гарантирует, что старые блоки физически исчезли (выравнивание износа).
  /// Настоящее стирание даст шифрование файлов собственным ключом (защищённое хранилище): тогда достаточно удалить ключ.
  Future<void> deleteForever(String path) async {
    final f = File(path);
    if (!await f.exists()) return;
    final doomed = '$path$_doomed';
    try {
      await f.rename(doomed);
    } catch (_) {
      try { await f.delete(); } catch (_) {}
      return;
    }
    _idle = _idle.then((_) => _wipeAndDelete(doomed));
  }

  Future<void> _wipeAndDelete(String path) async {
    var done = false;
    try { done = await (wipe?.call(path) ?? Future.value(false)); } catch (_) {}
    if (!done) {
      try { final f = File(path); if (await f.exists()) await f.delete(); } catch (_) {}
    }
  }

  Future<void> deleteAll(Iterable<String> paths) async {
    for (final p in paths) { await deleteForever(p); }
  }

  /// Дочистка: файлы, которые начали удалять, но приложение закрыли раньше конца.
  Future<void> finishPendingWipes() async {
    if (!await dir.exists()) return;
    await for (final e in dir.list(followLinks: false)) {
      if (e is File && e.path.endsWith(_doomed)) _idle = _idle.then((_) => _wipeAndDelete(e.path));
    }
    await _idle;
  }
}
