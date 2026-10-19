import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/dashboard.dart';
import 'package:yandi_desktop/home_blocks.dart';

Widget host(Widget child) => MaterialApp(home: Scaffold(body: SingleChildScrollView(child: child)));

void main() {
  testWidgets('блок новостей: доступное обновление с описанием, очищенным от управляющих знаков и ограниченным по длине', (t) async {
    final notes = 'Исправлено\u0000 много\u0007 всего. ${'я' * 800}';
    await t.pumpWidget(host(NewsBlock(update: {'current': '0.2.0+g768', 'available': {'version': '0.3.0', 'notes': notes}})));
    expect(find.textContaining('Доступно обновление YANDI 0.3.0'), findsOneWidget);
    final shown = (t.widget(find.byKey(const ValueKey('news-notes'))) as Text).data!;
    expect(shown.contains('\u0000') || shown.contains('\u0007'), isFalse);
    expect(shown.length <= 601, isTrue, reason: 'описание обрезано до 600 знаков');
    expect(shown.endsWith('…'), isTrue);
  });

  testWidgets('блок новостей: обновлений нет — версия и спокойный текст; ссылка в описании остаётся простым текстом', (t) async {
    await t.pumpWidget(host(const NewsBlock(update: {'current': '0.2.0+g768', 'available': null})));
    expect(find.textContaining('Установлена версия 0.2.0+g768. Новых обновлений нет.'), findsOneWidget);
    await t.pumpWidget(host(const NewsBlock(update: {'current': '0.2', 'available': {'version': '0.3.0', 'notes': 'смотрите https://evil.example/x'}})));
    final notes = find.byKey(const ValueKey('news-notes'));
    expect(notes, findsOneWidget);
    expect(find.descendant(of: find.byKey(const ValueKey('news-block')), matching: find.byType(GestureDetector)), findsNothing, reason: 'ссылок-нажатий нет');
    await t.pumpWidget(host(const NewsBlock(update: null)));
    expect(find.textContaining('Здесь будут появляться новости'), findsWidgets);
  });

  test('разбор адресов: пустые, слишком короткие и с лишними знаками отбрасываются, число ограничено', () {
    final ok = 'bc1q' + 'a' * 30;
    final j = '{"wallets":[{"coin":"Bitcoin","network":"BTC","address":"$ok"},{"coin":"Empty","address":""},{"coin":"Short","address":"abc"},'
        '{"coin":"Bad","address":"${'a' * 25} <script>"},{"coin":"Spaces","address":"${'a' * 25} ${'b' * 10}"},5,{"coin":"","address":"$ok"}]}';
    final w = Wallet.parse(j);
    expect(w.map((x) => x.coin), ['Bitcoin']);
    expect(w.single.address, ok);
    expect(Wallet.parse('не json'), isEmpty);
    expect(Wallet.parse('{"wallets":5}'), isEmpty);
    final many = '{"wallets":[${List.generate(20, (i) => '{"coin":"C$i","address":"${'x' * 30}"}').join(',')}]}';
    expect(Wallet.parse(many).length, 8);
  });

  testWidgets('поддержка: адрес показывается и копируется; без адресов — понятное сообщение', (t) async {
    final copied = <String>[];
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') copied.add((call.arguments as Map)['text'] as String);
      return null;
    });
    addTearDown(() => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, null));
    final addr = 'bc1q' + 'z' * 30;
    await t.pumpWidget(host(SupportBlock(load: () async => '{"wallets":[{"coin":"Bitcoin","network":"BTC","address":"$addr"}]}')));
    await t.pumpAndSettle();
    expect(find.text('Поддержать проект'), findsOneWidget);
    expect(find.text(addr), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('copy-Bitcoin')));
    await t.pumpAndSettle();
    expect(copied, [addr]);
    expect(find.text('Скопировано'), findsOneWidget);
    await t.pumpWidget(host(SupportBlock(key: const ValueKey('s2'), load: () async => '{"wallets":[{"coin":"Bitcoin","address":""}]}')));
    await t.pumpAndSettle();
    expect(find.byKey(const ValueKey('support-empty')), findsOneWidget);
    // сбой чтения файла не ломает главную
    await t.pumpWidget(host(SupportBlock(key: const ValueKey('s3'), load: () async => throw StateError('нет файла'))));
    await t.pumpAndSettle();
    expect(find.byKey(const ValueKey('support-empty')), findsOneWidget);
  });

  testWidgets('главная: закреплённый блок новостей вверху, ниже «Поддержать проект», затем управление', (t) async {
    t.view.physicalSize = const Size(1400, 1800);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    await t.pumpWidget(host(const DashboardView(status: null, speed: null, nodes: [], controls: SizedBox(), update: {'current': '0.2.0', 'available': null})));
    await t.pumpAndSettle();
    final news = t.getTopLeft(find.byKey(const ValueKey('news-block'))).dy;
    final support = t.getTopLeft(find.byKey(const ValueKey('support-block'))).dy;
    final control = t.getTopLeft(find.text('Управление')).dy;
    expect(news < support, isTrue, reason: 'новости выше поддержки');
    expect(support < control, isTrue, reason: 'поддержка выше управления');
  });
}
