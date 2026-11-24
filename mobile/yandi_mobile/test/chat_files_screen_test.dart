import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/models/file_offer.dart';
import 'package:yandi_mobile/services/files_host.dart';
import 'package:yandi_mobile/screens/chat_files_screen.dart';
import 'package:yandi_mobile/services/file_transfer_service.dart';
import 'package:yandi_mobile/services/vault_files.dart';
import 'package:yandi_mobile/theme.dart';

class FakeHost extends ChangeNotifier implements FilesHost {
  @override
  List<Contact> contacts = [
    for (var i = 0; i < 25; i++)
      Contact(peerId: 'p${i.toString().padLeft(2, '0')}', displayName: 'Друг $i', online: i.isEven, isManual: false, saved: true),
  ];
  @override
  int transferCount = 0;
  final sent = <String>[];
  @override
  Future<String?> sendFile(String peerId, String path, {String? name}) async {
    sent.add('$peerId:${name ?? path}');
    return null;
  }
}

final _key = List<int>.generate(32, (i) => i + 3);
FileVault _fv() => FileVault.withKey(_key, chunkSize: 1024);

FileOffer _offer(int n, String name, String mime) => FileOffer(
    tid: n.toString().padLeft(32, '0'), name: name, size: 5, chunks: 1, mime: mime, key: 'k', sha: '');

void main() {
  late Directory dir;
  setUp(() => dir = Directory.systemTemp.createTempSync('yandi_chatfiles'));
  tearDown(() => dir.deleteSync(recursive: true));

  Future<void> io(WidgetTester t, [int rounds = 60]) async {
    for (var i = 0; i < rounds; i++) {
      await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 60)));
      await t.pump();
    }
  }

  Future<List<FileOffer>> seed(WidgetTester t) async {
    final offers = [
      _offer(1, 'отчёт.pdf', 'application/pdf'),
      _offer(2, 'песня.mp3', 'audio/mpeg'),
      _offer(3, 'фото.png', 'image/png'),
      _offer(4, 'архив.zip', 'application/zip'),
      _offer(5, 'нет-на-диске.txt', 'text/plain'),
    ];
    await t.runAsync(() async {
      for (final o in offers.take(4)) {
        final f = File('${dir.path}/${FileTransferService.localName(o)}');
        final w = await _fv().create(f, name: o.name, mime: o.mime, size: 5);
        await w.add([1, 2, 3, 4, 5]);
        await w.close();
      }
    });
    return offers;
  }

  Future<void> pump(WidgetTester t, List<FileOffer> offers, {FakeHost? host, int maxPerFile = 20, List<FileLink> links = const [], Future<bool> Function(FileOffer)? onFetch}) async {
    await t.pumpWidget(MaterialApp(
      theme: AppTheme.dark,
      home: ChatFilesScreen(
        title: 'Алиса',
        shortId: 'abcdef12',
        offers: offers,
        host: host ?? FakeHost(),
        maxPerFile: maxPerFile,
        links: links,
        onFetch: onFetch,
        vault: VaultFiles(dir, vault: () async => _fv(), tempDir: Directory('${dir.path}/tmp')),
      ),
    ));
    await io(t);
  }

  /// Строка получателя: список ленивый, часть строк за пределами экрана — выбираем напрямую, как это сделал бы тап.
  Future<void> rcpt(WidgetTester t, int n) async {
    final k = ValueKey('rcpt-p${n.toString().padLeft(2, '0')}');
    final f = find.byKey(k, skipOffstage: false);
    if (f.evaluate().isEmpty) fail('нет строки $k');
    t.widget<CheckboxListTile>(f).onChanged!(true);
    await t.pump();
  }

  test('file kinds by mime and extension', () {
    expect(fileKindOf('a.pdf', 'application/pdf'), FileKind.documents);
    expect(fileKindOf('a.mp3', 'audio/mpeg'), FileKind.music);
    expect(fileKindOf('a.mkv', 'video/x-matroska'), FileKind.video);
    expect(fileKindOf('a.jpg', 'image/jpeg'), FileKind.pictures);
    expect(fileKindOf('a.rar', 'application/octet-stream'), FileKind.archives);
    expect(fileKindOf('a.bin', 'application/octet-stream'), FileKind.other);
  });

  testWidgets('папка показывает только файлы чата по видам, с коротким идентификатором', (t) async {
    final offers = await seed(t);
    await pump(t, offers);
    expect(find.text('Файлы чата'), findsOneWidget);
    expect(find.textContaining('abcdef12'), findsOneWidget);
    expect(find.text('Документы · 1'), findsOneWidget);
    expect(find.text('Музыка · 1'), findsOneWidget);
    expect(find.text('Картинки · 1'), findsOneWidget);
    expect(find.text('Архивы · 1'), findsOneWidget);
    expect(find.text('отчёт.pdf'), findsOneWidget);
    expect(find.text('нет-на-диске.txt'), findsNothing);
  });

  testWidgets('выбор нескольких и удаление с подтверждением', (t) async {
    final offers = await seed(t);
    await pump(t, offers);
    await t.longPress(find.text('отчёт.pdf'));
    await t.pumpAndSettle();
    await t.tap(find.text('Выбрать'));
    await t.pumpAndSettle();
    await t.tap(find.text('песня.mp3'));
    await t.pump();
    expect(find.text('Выбрано: 2'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('files-delete-selected')));
    await t.pumpAndSettle();
    expect(find.text('Удалить файлы (2)?'), findsOneWidget);
    await t.tap(find.text('Да'));
    await io(t);
    expect(find.text('отчёт.pdf'), findsNothing);
    expect(find.text('песня.mp3'), findsNothing);
    expect(find.text('фото.png'), findsOneWidget);
  });

  testWidgets('корзина: очистить папку — Уверены? Нет оставляет, Да очищает', (t) async {
    final offers = await seed(t);
    await pump(t, offers);
    await t.tap(find.byKey(const ValueKey('files-trash')));
    await t.pumpAndSettle();
    expect(find.text('Очистить папку'), findsOneWidget);
    expect(find.text('Отмена'), findsOneWidget);
    await t.tap(find.text('Очистить папку'));
    await t.pumpAndSettle();
    expect(find.text('Уверены?'), findsOneWidget);
    await t.tap(find.text('Нет'));
    await t.pumpAndSettle();
    expect(find.text('отчёт.pdf'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('files-trash')));
    await t.pumpAndSettle();
    await t.tap(find.text('Очистить папку'));
    await t.pumpAndSettle();
    await t.tap(find.text('Да'));
    await io(t);
    expect(find.textContaining('пока нет файлов'), findsOneWidget);
  });

  testWidgets('удержание файла: меню Переслать / Выбрать / Удалить, удаление с подтверждением', (t) async {
    final offers = await seed(t);
    await pump(t, offers);
    await t.longPress(find.text('фото.png'));
    await t.pumpAndSettle();
    expect(find.text('Переслать'), findsOneWidget);
    expect(find.text('Выбрать'), findsOneWidget);
    expect(find.text('Удалить'), findsOneWidget);
    await t.tap(find.text('Удалить'));
    await t.pumpAndSettle();
    expect(find.text('Удалить файл?'), findsOneWidget);
    await t.tap(find.text('Нет'));
    await t.pumpAndSettle();
    expect(find.text('фото.png'), findsOneWidget);
    await t.longPress(find.text('фото.png'));
    await t.pumpAndSettle();
    await t.tap(find.text('Удалить'));
    await t.pumpAndSettle();
    await t.tap(find.text('Да'));
    await io(t);
    expect(find.text('фото.png'), findsNothing);
  });

  testWidgets('переслать из меню: список с прокруткой, до 5 получателей за раз', (t) async {
    final offers = await seed(t);
    final host = FakeHost();
    await t.binding.setSurfaceSize(const Size(800, 3200));
    addTearDown(() => t.binding.setSurfaceSize(null));
    await pump(t, offers, host: host);
    await t.longPress(find.text('отчёт.pdf'));
    await t.pumpAndSettle();
    await t.tap(find.text('Переслать'));
    await t.pumpAndSettle();
    expect(find.text('Переслать «отчёт.pdf»'), findsOneWidget);
    for (final n in [0, 10, 12, 14, 16]) {
      await rcpt(t, n);
    }
    // шестого выбрать нельзя
    await rcpt(t, 18);
    expect(find.text('Не больше 5 получателей за раз'), findsOneWidget);
    expect(find.text('Отправить (5)'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('rcpt-send')));
    await t.pumpAndSettle();
    expect(host.sent.length, 5);
    expect(host.sent.first.endsWith(':отчёт.pdf'), true);
  });

  testWidgets('несколько файлов конвертом, предел получателей на файл', (t) async {
    final offers = await seed(t);
    final host = FakeHost();
    await pump(t, offers, host: host, maxPerFile: 7);
    Future<void> selectBoth() async {
      await t.longPress(find.text('отчёт.pdf'));
      await t.pumpAndSettle();
      await t.tap(find.text('Выбрать'));
      await t.pumpAndSettle();
      await t.tap(find.text('песня.mp3'));
      await t.pump();
    }
    await selectBoth();
    await t.tap(find.byKey(const ValueKey('files-forward-selected')));
    await t.pumpAndSettle();
    for (final n in [0, 10, 12, 14, 16]) {
      await rcpt(t, n);
    }
    await t.tap(find.byKey(const ValueKey('rcpt-send')));
    await t.pumpAndSettle();
    expect(host.sent.length, 5 * 2);
    // ещё трое новых: всего 8 > 7 — отказ, ничего не отправлено
    await selectBoth();
    await t.tap(find.byKey(const ValueKey('files-forward-selected')));
    await t.pumpAndSettle();
    for (final n in [18, 2, 20]) {
      await rcpt(t, n);
    }
    await t.tap(find.byKey(const ValueKey('rcpt-send')));
    await t.pumpAndSettle();
    expect(host.sent.length, 10);
    expect(find.textContaining('не больше 7 получателей'), findsOneWidget);
  });

  testWidgets('вкладки по видам, ссылки в своём разделе, сообщений в папке нет', (t) async {
    final offers = await seed(t);
    await pump(t, offers, links: [FileLink('https://example.org/a', DateTime(2026, 10, 1))]);
    expect(find.text('отчёт.pdf'), findsOneWidget);
    expect(find.text('https://example.org/a'), findsOneWidget);
    await t.tap(find.text('Картинки'));
    await t.pump();
    expect(find.text('фото.png'), findsOneWidget);
    expect(find.text('отчёт.pdf'), findsNothing);
    expect(find.text('https://example.org/a'), findsNothing);
    await t.tap(find.text('Аудио'));
    await t.pump();
    expect(find.text('песня.mp3'), findsOneWidget);
    await t.tap(find.text('Видео'));
    await t.pump();
    expect(find.text('В этом разделе пусто'), findsOneWidget);
    await t.tap(find.text('Ссылки'));
    await t.pump();
    expect(find.text('https://example.org/a'), findsOneWidget);
    expect(find.text('песня.mp3'), findsNothing);
  });

  testWidgets('файл группы, которого ещё нет у меня, виден как «не скачан» и скачивается по нажатию', (t) async {
    final offers = await seed(t);
    final asked = <String>[];
    await pump(t, offers, onFetch: (o) async { asked.add(o.name); return true; });
    expect(find.text('нет-на-диске.txt'), findsOneWidget);
    expect(find.textContaining('не скачан'), findsOneWidget);
    await t.tap(find.text('нет-на-диске.txt'));
    await io(t, 5);
    expect(asked, ['нет-на-диске.txt']);
  });

  test('ссылки из текста: адрес без хвостовой точки', () {
    final l = FileLink.inText('смотри https://a.b/c, и www.x.y.', DateTime(2026));
    expect(l.map((e) => e.url), ['https://a.b/c', 'www.x.y']);
  });
}
