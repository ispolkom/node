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

  test('удаление: файл исчезает сразу, а перед этим его содержимое затёрто', () async {
    final d = Directory('${dir.path}/yandi_downloads')..createSync();
    final secret = Uint8List.fromList(List<int>.filled(200000, 0x41));
    final f = File('${d.path}/dddddddd_secret.bin')..writeAsBytesSync(secret);
    // держим открытый дескриптор, чтобы увидеть, что осталось в файле ПЕРЕД удалением (на Linux он читаем после unlink)
    final raf = f.openSync();
    await vault.deleteForever(f.path);
    expect(f.existsSync(), isFalse);
    raf.setPositionSync(0);
    final left = raf.readSync(200000);
    raf.closeSync();
    expect(left.where((b) => b == 0x41).length, lessThan(2000), reason: 'содержимое должно быть затёрто случайными байтами');
    expect(await vault.list(), isEmpty);
    await vault.deleteForever('${d.path}/нет_такого'); // отсутствующий файл — не ошибка
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
