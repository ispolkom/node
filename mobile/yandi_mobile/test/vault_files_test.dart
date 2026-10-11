import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/services/vault_files.dart';

final _key = List<int>.generate(32, (i) => i + 3);
FileVault _fv() => FileVault.withKey(_key, chunkSize: 1024);

/// Зашифрованный файл-образец прямо в папке хранилища.
Future<File> _enc(Directory d, String id, String name, List<int> data, {int? createdMs}) async {
  final f = File('${d.path}/$id.yfe');
  final w = await _fv().create(f, name: name, mime: 'x', size: data.length, createdMs: createdMs);
  await w.add(data);
  await w.close();
  return f;
}

const _a = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', _b = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', _c = 'cccccccccccccccccccccccccccccccc';

void main() {
  late Directory dir, d;
  late VaultFiles vault;
  VaultFiles mk({Future<bool> Function(String)? wipe}) => VaultFiles(d, vault: () async => _fv(), wipe: wipe, tempDir: Directory('${dir.path}/tmp'));
  setUp(() {
    dir = Directory.systemTemp.createTempSync('yandi_vault');
    d = Directory('${dir.path}/yandi_downloads')..createSync();
    vault = mk();
  });
  tearDown(() => dir.deleteSync(recursive: true));

  test('список: новые сверху, недокачанное и повреждённое не показывается, имя и тип берутся изнутри файла', () async {
    await _enc(d, _a, 'old.mp3', [1, 2, 3], createdMs: DateTime(2020).millisecondsSinceEpoch);
    await _enc(d, _b, 'new.png', [1, 2], createdMs: DateTime(2026).millisecondsSinceEpoch);
    File('${d.path}/$_c.yfe.part').writeAsBytesSync([9]);
    File('${d.path}/dddddddddddddddddddddddddddddddd.yfe').writeAsBytesSync(List.filled(100, 7));   // не наш формат
    final l = await vault.list();
    expect(l.map((f) => f.name), ['new.png', 'old.mp3']);
    expect(l.first.size, 2);
    expect(l.first.id, _b);
    // на диске имён нет
    expect(d.listSync().map((e) => e.uri.pathSegments.last).any((n) => n.contains('png') || n.contains('mp3')), isFalse);
  });

  test('импорт делает зашифрованную копию, исходный файл остаётся, на диске нет ни имени, ни текста', () async {
    final src = File('${dir.path}/doc.txt')..writeAsStringSync('привет, секретный мир');
    final v = await vault.importFile(src.path);
    expect(v.name, 'doc.txt');
    expect(v.mime, 'text/plain');
    expect(RegExp(r'^[0-9a-f]{32}\.yfe$').hasMatch(v.path.split('/').last), isTrue);
    expect(await _fv().readAll(File(v.path)), src.readAsBytesSync());
    final raw = File(v.path).readAsBytesSync();
    expect(String.fromCharCodes(raw), isNot(contains('doc.txt')));
    expect(src.existsSync(), isTrue);
    expect((await vault.list()).length, 1);
  });

  test('импорт под заданным номером; чужой номер с путями сводится к безопасному имени', () async {
    final src = File('${dir.path}/p.bin')..writeAsBytesSync([1, 2, 3, 4]);
    final v = await vault.importAs(src.path, _a, name: 'снимок.jpg');
    expect(v.path, '${d.path}/$_a.yfe');
    expect(v.mime, 'image/jpeg');
    final evil = await vault.importAs(src.path, '../../etc/passwd', name: 'x.bin');
    expect(evil.path.startsWith('${d.path}/'), isTrue);
    expect(RegExp(r'^[0-9a-f]{32}\.yfe$').hasMatch(evil.path.split('/').last), isTrue);
    expect(Directory(dir.path).listSync(recursive: true).whereType<File>().every((f) => f.path.startsWith(d.path) || f.path.startsWith('${dir.path}/p.bin')), isTrue);
  });

  test('временная расшифрованная копия: есть на время работы, потом затёрта (и при ошибке тоже)', () async {
    final src = File('${dir.path}/r.pdf')..writeAsBytesSync(List.generate(2500, (i) => i % 251));
    final v = await vault.importFile(src.path);
    String? seen;
    final r = await vault.withPlain(v.path, (plain) async {
      seen = plain;
      expect(File(plain).readAsBytesSync(), src.readAsBytesSync());
      expect(plain.endsWith('.pdf'), isTrue);
      return 42;
    });
    expect(r, 42);
    expect(File(seen!).existsSync(), isFalse);
    await expectLater(vault.withPlain(v.path, (plain) async {
      seen = plain;
      throw StateError('сбой');
    }), throwsStateError);
    expect(File(seen!).existsSync(), isFalse);
    // остаток прошлого сеанса чистится при входе
    final tmp = Directory('${dir.path}/tmp')..createSync();
    File('${tmp.path}/left.bin').writeAsBytesSync([1]);
    await vault.cleanTemp();
    expect(tmp.listSync(), isEmpty);
  });

  test('удаление: файл мгновенно пропадает из списка, затирание и удаление идут в фоне', () async {
    final f = await _enc(d, _a, 'secret.bin', List<int>.filled(1000, 0x41));
    final wiped = <String>[];
    final v = mk(wipe: (p) async { wiped.add(p); await File(p).delete(); return true; });
    await v.deleteForever(f.path);
    expect(f.existsSync(), isFalse);
    expect(await v.list(), isEmpty, reason: 'из списка ушёл сразу');
    await v.idle;
    expect(wiped, ['${f.path}.wipe'], reason: 'нативному затиранию отдан переименованный файл');
    expect(d.listSync(), isEmpty);
    await v.deleteForever('${d.path}/нет_такого'); // отсутствующий файл — не ошибка
  });

  test('если затирание не вышло, файл не удаляется молча: скрыт из списка и дочищается при следующем запуске', () async {
    final f = await _enc(d, _a, 'x.bin', [1]);
    final v = mk(wipe: (p) async => false);
    await v.deleteForever(f.path);
    await v.idle;
    expect(await v.list(), isEmpty, reason: 'в хранилище его уже не видно');
    expect(d.listSync().map((e) => e.uri.pathSegments.last), ['$_a.yfe.wipe'], reason: 'но файл не брошен без затирания');
    final later = mk(wipe: (p) async { await File(p).delete(); return true; });
    await later.finishPendingWipes();
    expect(d.listSync(), isEmpty);
    final g = await _enc(d, _b, 'y.bin', [2]);
    await vault.deleteForever(g.path);   // без нативного затирания вовсе
    await vault.idle;
    expect(d.listSync(), isEmpty);
  });

  test('недоделанное удаление дочищается, а в списке не видно', () async {
    File('${d.path}/$_a.yfe.wipe').writeAsStringSync('остаток');
    await _enc(d, _b, 'ok.txt', [1, 2]);
    final v = mk(wipe: (p) async { await File(p).delete(); return true; });
    expect((await v.list()).map((f) => f.name), ['ok.txt']);
    await v.finishPendingWipes();
    expect(d.listSync().map((e) => e.uri.pathSegments.last), ['$_b.yfe']);
  });

  test('deleteAll удаляет всё выбранное', () async {
    final a = await _enc(d, _a, 'a', [1]);
    final b = await _enc(d, _b, 'b', [2]);
    await _enc(d, _c, 'keep', [3]);
    await vault.deleteAll([a.path, b.path]);
    expect((await vault.list()).map((f) => f.name), ['keep']);
  });

  test('перенос старых открытых файлов: шифруются, оригиналы стираются, недокачанное старого вида удаляется, повтор ничего не делает', () async {
    File('${d.path}/aaaaaaaa_old.txt').writeAsStringSync('старый текст');
    File('${d.path}/bbbbbbbb_pic.png').writeAsBytesSync([1, 2, 3]);
    File('${d.path}/cccccccc_half.bin.part').writeAsBytesSync([9]);
    await _enc(d, _a, 'уже.зашифрован', [5]);
    File('${d.path}/$_b.yfe.part').writeAsBytesSync([8]);   // идущая загрузка нового вида не трогается
    expect(await vault.migrateLegacy(), 2);
    final names = (await vault.list()).map((f) => f.name).toSet();
    expect(names, {'old.txt', 'pic.png', 'уже.зашифрован'});
    final old = (await vault.list()).firstWhere((f) => f.name == 'old.txt');
    expect(utf8.decode(await _fv().readAll(File(old.path))), 'старый текст');
    final left = d.listSync().map((e) => e.uri.pathSegments.last).toList();
    expect(left.any((n) => n.contains('old') || n.contains('pic') || n.contains('half')), isFalse, reason: '$left');
    expect(left, contains('$_b.yfe.part'));
    expect(await vault.migrateLegacy(), 0);
  });

  test('перенос сохраняет связь с чатом и группой и не плодит копии при сбое затирания (R12-14, R12-15)', () async {
    File('${d.path}/abcdef12_отчёт.pdf').writeAsBytesSync([1, 2, 3, 4, 5]);
    File('${d.path}/99999999_ещё.txt').writeAsBytesSync([7, 7]);
    const tid = 'abcdef12aaaaaaaaaaaaaaaaaaaaaaaa';
    // затирание сначала не удаётся: оригиналы остаются, копии не размножаются
    final stuck = mk(wipe: (p) async => false);
    Future<String?> resolve(String path, String name) async => name.startsWith('abcdef12_') ? tid : null;
    expect(await stuck.migrateLegacy(resolveId: resolve), 2);
    expect(stuck.migrationFailures, 2, reason: 'оригиналы не затёрты — об этом надо сказать');
    expect(File('${d.path}/$tid.yfe').existsSync(), isTrue, reason: 'чат найдёт файл по номеру передачи');
    final copies1 = d.listSync().where((e) => e.path.endsWith('.yfe')).length;
    expect(await stuck.migrateLegacy(resolveId: resolve), 0, reason: 'повтор: копия уже есть, второй не создаётся');
    expect(d.listSync().where((e) => e.path.endsWith('.yfe')).length, copies1);
    // затирание заработало: оригиналы уходят, копии те же
    final ok = mk(wipe: (p) async { await File(p).delete(); return true; });
    expect(await ok.migrateLegacy(resolveId: resolve), 0);
    expect(ok.migrationFailures, 0);
    expect(d.listSync().whereType<File>().where((f) => !f.path.endsWith('.yfe')), isEmpty);
    expect((await ok.list()).map((f) => f.name).toSet(), {'отчёт.pdf', 'ещё.txt'});
  });

  test('если зашифровать не вышло, единственная открытая копия остаётся и это считается (R12-15)', () async {
    final f = File('${d.path}/12345678_плохой.bin')..writeAsBytesSync([1, 2, 3]);
    final broken = VaultFiles(d, vault: () async => throw StateError('нет ключа'), tempDir: Directory('${dir.path}/tmp'));
    expect(await broken.migrateLegacy(), 0);
    expect(broken.migrationFailures, 1);
    expect(f.existsSync(), isTrue);
  });
}
