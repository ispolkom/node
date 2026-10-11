import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';

const _cs = 1024;                                   // для тестов куски по 1 КБ
final _key = List<int>.generate(32, (i) => i * 7 + 1);

Uint8List _data(int n, [int seed = 1]) {
  final r = Random(seed);
  return Uint8List.fromList(List.generate(n, (_) => r.nextInt(256)));
}

void main() {
  late Directory tmp;
  late FileVault v;
  setUp(() {
    tmp = Directory.systemTemp.createTempSync('fv');
    v = FileVault.withKey(_key, chunkSize: _cs);
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<File> put(Uint8List d, {String name = 'Паспорт.pdf', String mime = 'application/pdf'}) async {
    final f = File('${tmp.path}/a${d.length}.yfe');
    final w = await v.create(f, name: name, mime: mime, size: d.length);
    // подаём неровными порциями
    var o = 0;
    while (o < d.length) {
      final n = min(777, d.length - o);
      await w.add(d.sublist(o, o + n));
      o += n;
    }
    await w.close();
    return f;
  }

  test('туда-обратно на границах размера куска, сведения читаются без расшифровки содержимого', () async {
    for (final n in [0, 1, _cs - 1, _cs, _cs + 1, 2 * _cs, 2 * _cs + 5, 5 * _cs + 333]) {
      final d = _data(n, n + 3);
      final f = await put(d);
      final m = await v.readMeta(f);
      expect((m.name, m.mime, m.size), ('Паспорт.pdf', 'application/pdf', n), reason: 'размер $n');
      expect(await v.readAll(f), d, reason: 'размер $n');
      final streamed = BytesBuilder();
      await for (final c in v.read(f)) {
        streamed.add(c);
      }
      expect(streamed.toBytes(), d);
    }
  });

  test('чтение с любого места и текстовый предпросмотр', () async {
    final d = _data(5 * _cs + 100);
    final f = await put(d);
    for (final (pos, len) in [(0, 10), (_cs - 5, 10), (_cs, _cs), (3 * _cs + 7, 2 * _cs), (5 * _cs + 90, 500), (0, 99999)]) {
      final end = min(pos + len, d.length);
      expect(await v.readRange(f, pos, len), d.sublist(pos, end), reason: '$pos+$len');
    }
    expect(await v.readRange(f, d.length + 5, 10), isEmpty);
    expect(await v.readPrefix(f, 300), d.sublist(0, 300));
  });

  test('на диске нет ни имени, ни содержимого; одинаковое содержимое шифруется по-разному', () async {
    final d = Uint8List.fromList(List.filled(3000, 0x41));   // ААА…
    final f = await put(d, name: 'секретный отчёт.docx');
    final raw = f.readAsBytesSync();
    expect(String.fromCharCodes(raw).contains('AAAAAAAAAAAAAAAA'), isFalse);
    expect(String.fromCharCodes(raw), isNot(contains('docx')));
    expect(String.fromCharCodes(raw), isNot(contains('отчёт')));
    final g = File('${tmp.path}/b.yfe');
    final w = await v.create(g, name: 'секретный отчёт.docx', mime: 'x', size: d.length);
    await w.add(d);
    await w.close();
    expect(g.readAsBytesSync(), isNot(raw));
  });

  test('чужой ключ и порча заголовка, куска или метки не открываются', () async {
    final d = _data(3 * _cs + 10);
    final f = await put(d);
    await expectLater(FileVault.withKey(List.filled(32, 9), chunkSize: _cs).readAll(f), throwsA(isA<FileVaultException>()));
    final raw = f.readAsBytesSync();
    for (final pos in [5, 10, 26, 30, 36, raw.length ~/ 2, raw.length - 1]) {
      f.writeAsBytesSync(Uint8List.fromList(raw)..[pos] ^= 1);   // под тем же именем: ошибка именно от порчи, а не от чужого имени
      await expectLater(v.readAll(f), throwsA(isA<FileVaultException>()), reason: 'байт $pos');
    }
    f.writeAsBytesSync(raw);
    expect(await v.readAll(f), d, reason: 'нетронутый файл открывается');
  });

  test('обрезка, дописывание и перестановка кусков обнаруживаются', () async {
    final d = _data(3 * _cs);
    final f = await put(d);
    final raw = f.readAsBytesSync();
    f.writeAsBytesSync(raw.sublist(0, raw.length - (_cs + 16)));   // без последнего куска
    await expectLater(v.readAll(f), throwsA(isA<FileVaultException>()));
    f.writeAsBytesSync([...raw, 1, 2, 3]);
    await expectLater(v.readAll(f), throwsA(isA<FileVaultException>()));
    // меняем местами 1-й и 2-й куски (одинаковой длины)
    final meta = raw.length - 3 * (_cs + 16);
    final c1 = raw.sublist(meta, meta + _cs + 16), c2 = raw.sublist(meta + _cs + 16, meta + 2 * (_cs + 16));
    final sw = Uint8List.fromList(raw)..setRange(meta, meta + _cs + 16, c2)..setRange(meta + _cs + 16, meta + 2 * (_cs + 16), c1);
    f.writeAsBytesSync(sw);
    await expectLater(v.readAll(f), throwsA(isA<FileVaultException>()));
  });

  test('неверный размер при записи: файл не остаётся', () async {
    final f = File('${tmp.path}/short.yfe');
    var w = await v.create(f, name: 'x', mime: 'x', size: 5000);
    await w.add(_data(4000));
    await expectLater(w.close(), throwsA(isA<FileVaultException>()));
    expect(f.existsSync(), isFalse);
    final g = File('${tmp.path}/long.yfe');
    w = await v.create(g, name: 'x', mime: 'x', size: 100);
    await expectLater(w.add(_data(101)), throwsA(isA<FileVaultException>()));
    expect(g.existsSync(), isFalse);
    final h = File('${tmp.path}/abort.yfe');
    w = await v.create(h, name: 'x', mime: 'x', size: 100);
    await w.add(_data(50));
    await w.abort();
    expect(h.existsSync(), isFalse);
  });

  test('импорт обычного файла и расшифровка во временный', () async {
    final d = _data(3 * _cs + 1);
    final plain = File('${tmp.path}/plain.bin')..writeAsBytesSync(d);
    final enc = File('${tmp.path}/${'ab' * 16}.yfe');
    final m = await v.importFile(plain, enc, name: 'снимок.jpg', mime: 'image/jpeg');
    expect(m.size, d.length);
    expect(FileVault.idOf(enc.path), 'ab' * 16);
    final back = File('${tmp.path}/back.bin');
    await v.exportTo(enc, back);
    expect(back.readAsBytesSync(), d);
    // обычный файл или мусор не принимается за файл хранилища
    await expectLater(v.readMeta(plain), throwsA(isA<FileVaultException>()));
  });

  test('источник для отправки: обычный и зашифрованный файлы читаются одинаково, в том числе мелкими шагами', () async {
    final d = _data(4 * _cs + 321);
    final plain = File('${tmp.path}/p.bin')..writeAsBytesSync(d);
    final enc = await put(d);
    for (final src in [await FileSource.open(plain.path), await FileSource.open(enc.path, vault: v)]) {
      expect(src.length, d.length);
      final got = BytesBuilder();
      for (var pos = 0; pos < d.length; pos += 300) {
        got.add(await src.readAt(pos, 300));
      }
      expect(got.toBytes(), d);
      expect(await src.readAt(_cs - 10, 40), d.sublist(_cs - 10, _cs + 30));
      await src.close();
    }
    await expectLater(FileSource.open(enc.path), throwsA(isA<FileVaultException>()), reason: 'без ключа зашифрованный не открыть');
  });

  test('контейнер привязан к своему номеру хранения: целиком скопированный под чужое имя не открывается (R12-20)', () async {
    final a = await put(_data(2000, 1), name: 'один.bin');
    final other = File('${tmp.path}/b${'0' * 8}.yfe');
    other.writeAsBytesSync(a.readAsBytesSync());                   // валидный контейнер, но под другим именем
    await expectLater(v.readAll(other), throwsA(isA<FileVaultException>()));
    await expectLater(v.readMeta(other), throwsA(isA<FileVaultException>()));
    // недокачанный файл несёт номер будущего: после переименования открывается
    final part = File('${tmp.path}/fin.yfe.part');
    final w = await v.create(part, name: 'x.bin', mime: 'x', size: 10);
    await w.add(_data(10));
    await w.close();
    final fin = await part.rename('${tmp.path}/fin.yfe');
    expect((await v.readMeta(fin)).size, 10);
  });

  test('блокировка отзывает уже открытые потоки, источники и записи (R12-5)', () async {
    final d = _data(5 * _cs);
    final f = await put(d);
    // поток: прочитан кусок, ключ стёрт — следующий кусок не отдаётся
    final it = StreamIterator(v.read(f));
    expect(await it.moveNext(), isTrue);
    v.dispose();
    await expectLater(() async => await it.moveNext(), throwsA(isA<FileVaultException>()));
    // новый объект с тем же ключом (другой сеанс) читает нормально
    final v2 = FileVault.withKey(_key, chunkSize: _cs);
    // источник с кешем куска
    final src = await FileSource.open(f.path, vault: v2);
    expect((await src.readAt(0, 10)).length, 10);
    v2.dispose();
    await expectLater(src.readAt(0, 10), throwsA(isA<FileVaultException>()), reason: 'кеш не отдаётся после блокировки');
    // запись: блокировка посреди — файл не остаётся
    final v3 = FileVault.withKey(_key, chunkSize: _cs);
    final out = File('${tmp.path}/w.yfe');
    final w = await v3.create(out, name: 'x', mime: 'x', size: 4 * _cs);
    await w.add(_data(_cs + 5));
    v3.dispose();
    await expectLater(w.add(_data(100)), throwsA(isA<FileVaultException>()));
    expect(out.existsSync(), isFalse);
    // ключ стёрт в самом объекте, даже если его передали обычным списком
    final plainKey = List<int>.from(_key);
    final v4 = FileVault.withKey(plainKey, chunkSize: _cs);
    v4.dispose();
    await expectLater(v4.readMeta(f), throwsA(isA<FileVaultException>()));
  });
}
