import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/screens/files_tab.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/services/files_host.dart';
import 'package:yandi_mobile/services/vault_files.dart';
import 'package:yandi_mobile/theme.dart';

class FakeHost extends ChangeNotifier implements FilesHost {
  @override
  List<Contact> contacts = [Contact(peerId: 'p1', displayName: 'Дом', online: true, isManual: false)];
  @override
  int transferCount = 0;
  final sent = <String>[];
  @override
  Future<String?> sendFile(String peerId, String path, {String? name}) async {
    sent.add('$peerId:${name ?? path}');
    return null;
  }
}

void main() {
  late Directory dir;
  late FakeHost host;
  setUp(() { dir = Directory.systemTemp.createTempSync('yandi_tab'); host = FakeHost(); });
  tearDown(() => dir.deleteSync(recursive: true));

  /// Дисковый ввод-вывод в flutter_test идёт только внутри runAsync; между такими паузами кадры прокручиваются вручную.
  Future<void> io(WidgetTester t, [int rounds = 6]) async {
    for (var i = 0; i < rounds; i++) {
      await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 60)));
      await t.pump();
    }
  }

  Future<void> pump(WidgetTester t) async {
    await t.pumpWidget(MaterialApp(theme: AppTheme.dark, home: Scaffold(body: FilesTab(vault: VaultFiles(dir), host: host))));
    await io(t);
  }

  testWidgets('пустое хранилище: понятная подсказка', (t) async {
    await pump(t);
    expect(find.text('Файлов пока нет'), findsOneWidget);
  });

  testWidgets('файлы показываются без служебного префикса, новые сверху', (t) async {
    File('${dir.path}/aaaaaaaa_old.txt')..writeAsStringSync('a')..setLastModifiedSync(DateTime(2020));
    File('${dir.path}/bbbbbbbb_new.bin')..writeAsBytesSync([1, 2, 3])..setLastModifiedSync(DateTime(2026));
    await pump(t);
    expect(find.text('old.txt'), findsOneWidget);
    expect(find.text('new.bin'), findsOneWidget);
    expect(find.textContaining('aaaaaaaa'), findsNothing);
    expect(t.getTopLeft(find.text('new.bin')).dy, lessThan(t.getTopLeft(find.text('old.txt')).dy));
  });

  testWidgets('долгое нажатие выбирает; удаление — с подтверждением, файл исчезает с диска', (t) async {
    final a = File('${dir.path}/11111111_a.txt')..writeAsStringSync('a');
    final b = File('${dir.path}/22222222_b.txt')..writeAsStringSync('b');
    await pump(t);
    await t.longPress(find.text('a.txt'));
    await t.pump();
    expect(find.text('Выбрано: 1'), findsOneWidget);
    await t.tap(find.text('b.txt'));      // при выбранном обычное нажатие добавляет в выбор
    await t.pump();
    expect(find.text('Выбрано: 2'), findsOneWidget);
    await t.tap(find.text('Удалить'));
    await t.pumpAndSettle();
    expect(find.textContaining('насовсем, без корзины'), findsOneWidget);
    expect(a.existsSync() && b.existsSync(), isTrue, reason: 'до подтверждения ничего не удалено');
    await t.tap(find.widgetWithText(TextButton, 'Удалить').last);
    // у каждого файла несколько шагов дискового ввода-вывода (затирание, закрытие, удаление): ждём по условию, а не по счёту
    for (var i = 0; i < 120 && (a.existsSync() || b.existsSync()); i++) { await io(t, 1); }
    await io(t, 8);
    expect(a.existsSync() || b.existsSync(), isFalse);
    expect(find.text('Файлов пока нет'), findsOneWidget);
  });

  testWidgets('отмена подтверждения ничего не удаляет', (t) async {
    final a = File('${dir.path}/33333333_keep.txt')..writeAsStringSync('k');
    await pump(t);
    await t.tap(find.byIcon(Icons.more_vert));
    await t.pumpAndSettle();
    await t.tap(find.text('Удалить'));
    await t.pumpAndSettle();
    await t.tap(find.text('Отмена'));
    await io(t);
    expect(a.existsSync(), isTrue);
    expect(find.text('keep.txt'), findsOneWidget);
  });

  testWidgets('пересылка: список контактов YANDI, отправка выбранному контакту', (t) async {
    File('${dir.path}/44444444_report.txt').writeAsStringSync('r');
    await pump(t);
    await t.tap(find.byIcon(Icons.more_vert));
    await t.pumpAndSettle();
    await t.tap(find.text('Переслать контакту'));
    await t.pumpAndSettle();
    expect(find.textContaining('Только внутри сети YANDI'), findsOneWidget);
    await t.tap(find.text('Дом'));
    await io(t);
    expect(host.sent, ['p1:report.txt']);
  });
}
