import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/vault_files.dart';

void main() {
  late Directory dir;
  late VaultFiles vault;
  setUp(() { dir = Directory.systemTemp.createTempSync('yandi_vault'); vault = VaultFiles(Directory('${dir.path}/yandi_downloads')); });
  tearDown(() => dir.deleteSync(recursive: true));

  test('служебный префикс убирается из имени для человека', () {
    expect(VaultFiles.displayName('281eea1a_1.jpg'), '1.jpg');
    expect(VaultFiles.displayName('photo_big.jpg'), 'photo_big.jpg');   // не наш префикс — не трогаем
  });

  test('список: новые сверху, недокачанное не показывается, тип по имени', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    File('${d.path}/aaaaaaaa_old.mp3').writeAsBytesSync([1, 2, 3]);
    await Future<void>.delayed(const Duration(milliseconds: 20));
    File('${d.path}/bbbbbbbb_new.png').writeAsBytesSync([1, 2]);
    File('${d.path}/cccccccc_half.bin.part').writeAsBytesSync([9]);
    final l = await vault.list();
    expect(l.map((f) => f.name), ['new.png', 'old.mp3']);
    expect(l.first.isImage, isTrue);
    expect(l.last.isAudio, isTrue);
    expect(l.first.size, 2);
  });

  test('импорт делает копию в хранилище, исходный файл остаётся', () async {
    final src = File('${dir.path}/doc.txt')..writeAsStringSync('привет');
    final v = await vault.importFile(src.path);
    expect(v.name, 'doc.txt');
    expect(File(v.path).readAsStringSync(), 'привет');
    expect(src.existsSync(), isTrue);
    expect((await vault.list()).length, 1);
  });

  test('удаление: файл мгновенно пропадает из списка, затирание и удаление идут в фоне', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    final f = File('${d.path}/dddddddd_secret.bin')..writeAsBytesSync(Uint8List.fromList(List<int>.filled(1000, 0x41)));
    final wiped = <String>[];
    final v = VaultFiles(d, wipe: (p) async { wiped.add(p); await File(p).delete(); return true; });
    await v.deleteForever(f.path);
    expect(f.existsSync(), isFalse);
    expect(await v.list(), isEmpty, reason: 'из списка ушёл сразу');
    await v.idle;
    expect(wiped, ['${f.path}.wipe'], reason: 'нативному затиранию отдан переименованный файл');
    expect(d.listSync(), isEmpty);
    await v.deleteForever('${d.path}/нет_такого'); // отсутствующий файл — не ошибка
  });

  test('если затирание не вышло, файл всё равно удаляется', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    final f = File('${d.path}/eeeeeeee_x.bin')..writeAsStringSync('x');
    final v = VaultFiles(d, wipe: (p) async => false);
    await v.deleteForever(f.path);
    await v.idle;
    expect(d.listSync(), isEmpty);
    final v2 = VaultFiles(d);   // без нативного затирания вовсе
    final g = File('${d.path}/ffffffff_y.bin')..writeAsStringSync('y');
    await v2.deleteForever(g.path);
    await v2.idle;
    expect(d.listSync(), isEmpty);
  });

  test('недоделанное удаление дочищается, а в списке не видно', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    File('${d.path}/aaaaaaaa_half.bin.wipe').writeAsStringSync('остаток');
    File('${d.path}/bbbbbbbb_ok.txt').writeAsStringSync('ok');
    final v = VaultFiles(d, wipe: (p) async { await File(p).delete(); return true; });
    expect((await v.list()).map((f) => f.name), ['ok.txt']);
    await v.finishPendingWipes();
    expect(d.listSync().map((e) => e.uri.pathSegments.last), ['bbbbbbbb_ok.txt']);
  });

  test('deleteAll удаляет всё выбранное', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    final a = File('${d.path}/11111111_a')..writeAsStringSync('a');
    final b = File('${d.path}/22222222_b')..writeAsStringSync('b');
    File('${d.path}/33333333_keep')..writeAsStringSync('k');
    await vault.deleteAll([a.path, b.path]);
    expect((await vault.list()).map((f) => f.name), ['keep']);
  });
}
