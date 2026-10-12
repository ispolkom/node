import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/models/file_offer.dart';
import 'package:yandi_mobile/screens/chat_files_screen.dart';
import 'package:yandi_mobile/services/file_transfer_service.dart';
import 'package:yandi_mobile/services/vault_files.dart';
import 'package:yandi_mobile/theme.dart';

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

  Future<void> pump(WidgetTester t, List<FileOffer> offers) async {
    await t.pumpWidget(MaterialApp(
      theme: AppTheme.dark,
      home: ChatFilesScreen(
        title: 'Алиса',
        shortId: 'abcdef12',
        offers: offers,
        vault: VaultFiles(dir, vault: () async => _fv(), tempDir: Directory('${dir.path}/tmp')),
      ),
    ));
    await io(t);
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
    await t.pump();
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
}
