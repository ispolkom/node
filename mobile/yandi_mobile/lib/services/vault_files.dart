import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'package:crypto/crypto.dart' show sha256;
import '../crypto/file_vault.dart';
import 'file_transfer_service.dart';

/// Файл в изолированном хранилище приложения (папка внутри песочницы Android, недоступная другим приложениям), зашифрованный.
/// На диске он называется безликим номером (`<32 hex>.yfe`); настоящее имя, тип и размер лежат внутри и видны только при входе.
class VaultFile {
  final String path;
  final String id;
  final String name;       // для человека
  final int size;
  final DateTime modified;
  final String mime;
  const VaultFile({required this.path, required this.id, required this.name, required this.size, required this.modified, required this.mime});

  bool get isImage => mime.startsWith('image/');
  bool get isAudio => mime.startsWith('audio/');
  bool get isVideo => mime.startsWith('video/');
  bool get isText => mime.startsWith('text/') || mime == 'application/json' || mime == 'application/xml';
}

/// Список, импорт и удаление файлов хранилища. Принятые и отправленные файлы лежат здесь же (`yandi_downloads`), так что это общий список.
class VaultFiles {
  final Directory dir;
  /// Шифрование файлов ключом этого сеанса (пока заблокировано — ошибка).
  final Future<FileVault> Function() vault;
  /// Затирание поверх байтов и удаление (на телефоне — нативное, `FileWipe`). Без него файл просто удаляется.
  final Future<bool> Function(String path)? wipe;
  /// Где лежат временные расшифрованные копии (на телефоне — кеш приложения); по умолчанию рядом, в `tmp`.
  final Directory? tempDir;
  VaultFiles(this.dir, {required this.vault, this.wipe, this.tempDir});

  /// Фоновые затирания (чтобы тест и интерфейс могли дождаться конца).
  Future<void> _idle = Future.value();
  Future<void> get idle => _idle;

  static const _doomed = '.wipe';
  static const _part = '.part';
  static final _legacyPrefix = RegExp(r'^[0-9a-f]{8}_');

  static String _randomId() {
    final r = Random.secure();
    return List.generate(16, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  }

  String pathFor(String id) => '${dir.path}/$id${FileVault.extension}';

  Future<List<VaultFile>> list() async {
    if (!await dir.exists()) return [];
    final v = await vault();
    final out = <VaultFile>[];
    await for (final e in dir.list(followLinks: false)) {
      if (e is! File || !FileVault.isVaultPath(e.path)) continue;    // недокачанное (.part) и уже удаляемое (.wipe) не показываем
      try {
        out.add(_describe(e.path, await v.readMeta(e)));
      } catch (_) {
        // файл не открывается (повреждён) — в списке его нет; удалить его можно через «удалить всё»
      }
    }
    out.sort((a, b) => b.modified.compareTo(a.modified));
    return out;
  }

  VaultFile _describe(String path, FileMeta m) => VaultFile(
        path: path, id: FileVault.idOf(path), name: m.name, size: m.size,
        modified: DateTime.fromMillisecondsSinceEpoch(m.createdMs), mime: m.mime,
      );

  /// Описание уже лежащего файла (например, полученного в чате) в том же виде, что и в списке.
  Future<VaultFile> describe(String path) async => _describe(path, await (await vault()).readMeta(File(path)));

  /// Положить зашифрованную копию файла в хранилище (чтобы потом переслать оттуда). Исходный файл не трогаем.
  Future<VaultFile> importFile(String sourcePath, {String? name}) =>
      importAs(sourcePath, _randomId(), name: name);

  /// То же с заданным номером (полученный файл — номер передачи, отправленный — тоже).
  Future<VaultFile> importAs(String sourcePath, String id, {String? name, String? mime}) async {
    final base = FileTransferService.safeName(name ?? sourcePath.split(Platform.pathSeparator).last);
    if (!await dir.exists()) await dir.create(recursive: true);
    final target = File(pathFor(FileVault.safeId(id)));
    final m = await (await vault()).importFile(File(sourcePath), target, name: base, mime: mime ?? FileTransferService.guessMime(base));
    return _describe(target.path, m);
  }

  /// Расшифрованная копия на время работы [use] (отправка, «вынести наружу»), затем она затирается. Если приложение убьют посреди
  /// работы, остаток чистится при следующем входе ([cleanTemp]).
  Future<T> withPlain<T>(String path, Future<T> Function(String plainPath) use) async {
    final t = await _tmp();
    final v = await vault();
    final meta = await v.readMeta(File(path));
    final ext = meta.name.contains('.') ? '.${meta.name.split('.').last.replaceAll(RegExp(r'[^A-Za-z0-9]'), '')}' : '';
    final f = File('${t.path}/${_randomId()}$ext');
    try {
      await v.exportTo(File(path), f);
      return await use(f.path);
    } finally {
      await _wipeAndDelete(f.path);
    }
  }

  Future<Directory> _tmp() async {
    final d = tempDir ?? Directory('${dir.path}/tmp');
    if (!await d.exists()) await d.create(recursive: true);
    return d;
  }

  /// Остатки временных расшифрованных копий (прошлый сеанс оборвался) затираются и удаляются.
  Future<void> cleanTemp() async {
    final d = tempDir ?? Directory('${dir.path}/tmp');
    if (!await d.exists()) return;
    await for (final e in d.list(followLinks: false)) {
      if (e is File) await _wipeAndDelete(e.path);
    }
  }

  /// Удаление сразу и насовсем, без корзины. Файл мгновенно пропадает из списка (переименование), а затирание поверх его байтов и
  /// удаление идут в фоне: на большом файле оно занимает время, и ждать его интерфейсу незачем. Недоделанное (закрыли приложение)
  /// дочищается при следующем открытии вкладки ([finishPendingWipes]).
  /// Содержимое и так зашифровано: без ключа данных остаток на флеш-памяти не прочесть (а после «удалить всё» ключа нет совсем).
  Future<void> deleteForever(String path) async {
    final f = File(path);
    if (!await f.exists()) return;
    var doomed = '$path$_doomed';
    // имя занято (остаток прошлого сбоя, в том числе каталог) — берём уникальное: просьба удалить не должна теряться
    if (await FileSystemEntity.type(doomed, followLinks: false) != FileSystemEntityType.notFound) doomed = '$path.${_randomId().substring(0, 8)}$_doomed';
    try {
      await f.rename(doomed);
    } catch (_) {
      // переименовать не вышло: затираем на месте; без затирания (не Android) — просто удаляем
      final w = wipe;
      if (w == null) {
        try { await f.delete(); } catch (_) {}
      } else {
        _idle = _idle.then((_) async { try { await w(path); } catch (_) {} });
      }
      return;
    }
    _idle = _idle.then((_) => _wipeAndDelete(doomed));
  }

  Future<void> _wipeAndDelete(String path) async {
    final w = wipe;
    if (w == null) {
      // затирать нечем (тесты, не Android): просто удаляем
      try { final f = File(path); if (await f.exists()) await f.delete(); } catch (_) {}
      return;
    }
    // затереть не вышло — файл не удаляем молча: он остаётся и дочищается при следующем запуске (finishPendingWipes)
    try { await w(path); } catch (_) {}
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

  /// Один раз после обновления: старые открытые файлы (`<8 hex>_<имя>`) шифруются, оригиналы затираются; открытые недокачанные
  /// остатки (`….part` старого вида) удаляются. Возвращает число перенесённых файлов; неудачи считаются в [migrationFailures].
  /// Перенос повторяемый: у каждого файла устойчивый номер назначения (по [resolveId], например номер передачи из чата, иначе по
  /// имени), уже зашифрованная копия не создаётся второй раз — повторяется только затирание оригинала.
  int migrationFailures = 0;

  Future<int> migrateLegacy({Future<String?> Function(String legacyPath, String legacyName)? resolveId}) async {
    migrationFailures = 0;
    if (!await dir.exists()) return 0;
    var moved = 0;
    final plain = <File>[];
    await for (final e in dir.list(followLinks: false)) {
      if (e is! File) continue;
      final p = e.path;
      if (p.endsWith(FileVault.extension) || p.endsWith('${FileVault.extension}$_part') || p.endsWith(_doomed)) continue;
      plain.add(e);
    }
    for (final f in plain) {
      final fileName = f.uri.pathSegments.last;
      if (fileName.endsWith(_part)) {
        await _wipeAndDelete(f.path);
        if (await f.exists()) migrationFailures++;
        continue;
      }
      try {
        final name = fileName.replaceFirst(_legacyPrefix, '');
        final id = FileVault.safeId((await resolveId?.call(f.path, fileName)) ?? _legacyId(fileName));
        final target = File(pathFor(id));
        var have = false;
        if (await target.exists()) {
          try {
            have = (await (await vault()).readMeta(target)).size == await f.length();
          } catch (_) {}
        }
        if (!have) {
          await importAs(f.path, id, name: name);
          moved++;
        }
        await _wipeAndDelete(f.path);
        if (await f.exists()) migrationFailures++;   // зашифровано, но оригинал затереть не вышло: останется до следующего входа
      } catch (_) {
        migrationFailures++;   // не вышло зашифровать — открытый файл остаётся (единственную копию не удаляем)
      }
    }
    return moved;
  }

  static String _legacyId(String fileName) => sha256.convert(utf8.encode('legacy|$fileName')).toString().substring(0, 32);
}
