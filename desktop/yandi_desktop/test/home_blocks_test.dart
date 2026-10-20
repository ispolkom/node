import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/bootstrap_panel.dart';
import 'package:yandi_desktop/home_blocks.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

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
    await t.tap(find.byKey(const ValueKey('copy-Bitcoin|BTC')));
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

  testWidgets('поддержка: кошельки из подписанного манифеста заменяют вшитые; мусор игнорируется', (t) async {
    const embedded = '{"wallets":[{"coin":"USDT","network":"Tron","address":"TPeLtLuE9qx64neroHn14LjWwRn5jmupHY"}]}';
    const evm = '0x00afCEFc2428B3D00f7d6B739C071339de4c2521';
    await t.pumpWidget(host(SupportBlock(load: () async => embedded, signed: const [
      {'asset': 'USDT/USDC', 'network': 'Polygon (PoS)', 'address': evm},
      {'asset': 'USDT/USDC', 'network': 'Ethereum (ERC-20)', 'address': evm},
    ])));
    await t.pump();
    expect(find.text(evm), findsNWidgets(2), reason: 'адрес EVM один, строк две (по сетям)');
    expect(find.text('TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'), findsNothing, reason: 'вшитый заменён');
    expect(find.byKey(const ValueKey('qr-USDT/USDC|Polygon (PoS)')), findsOneWidget);
    // неверные записи пропускаются; если верных нет — остаётся вшитый список
    await t.pumpWidget(host(SupportBlock(load: () async => embedded, signed: const [
      {'asset': 'USDT', 'network': 'x', 'address': 'bad address!'},
    ])));
    await t.pump();
    expect(find.text('TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'), findsOneWidget);
    await t.pumpWidget(host(SupportBlock(load: () async => embedded, signed: const {'asset': 'USDT'})));
    await t.pump();
    expect(find.text('TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'), findsOneWidget, reason: 'не список — игнорируется');
  });

  testWidgets('главная страница оболочки: сверху закреплённые новости и обновления, ниже «Поддержать проект» с адресом', (t) async {
    t.view.physicalSize = const Size(1400, 1800);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    await m2.open(t, m2.FeatureApi(), 'Главная');
    expect(find.byKey(const ValueKey('news-block')), findsOneWidget);
    expect(find.byKey(const ValueKey('support-block')), findsOneWidget);
    await t.pumpAndSettle();
    final news = t.getTopLeft(find.byKey(const ValueKey('news-block'))).dy;
    final support = t.getTopLeft(find.byKey(const ValueKey('support-block'))).dy;
    expect(news < support, isTrue, reason: 'новости выше поддержки');
    expect(find.text('TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'), findsOneWidget, reason: 'адрес виден и его можно скопировать');
    expect(find.byKey(const ValueKey('qr-USDT|Tron (TRC-20)')), findsOneWidget);
    await m1.stop(t);
  });

  testWidgets('панель входного списка: ключ подписи, заявки с результатом прозвона, копирование команды и отклонение; по-английски без русского', (t) async {
    String clip = '';
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') clip = (call.arguments as Map)['text'] as String;
      return null;
    });
    addTearDown(() => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, null));
    final api = _BootstrapApi();
    Widget panel(bool en) => PreferencesScope(
          preferences: AppPreferences()..english = en,
          child: MaterialApp(
            home: Scaffold(
              body: SingleChildScrollView(child: BootstrapPanel(api: api, busy: false, isActive: () => true, mutate: (a) => a())),
            ),
          ),
        );
    t.view.physicalSize = const Size(1400, 2400);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    await t.pumpWidget(panel(false));
    await t.pumpAndSettle();
    expect(find.textContaining('Открытый ключ подписи: ${'b' * 64}'), findsOneWidget);
    expect(find.text('Проверена: адрес ответил ключом узла'), findsOneWidget);
    expect(find.text('Узел 203.0.113.5:9000: отвечает'), findsOneWidget);
    expect(find.text('Телефон 203.0.113.5:8444: не отвечает'), findsOneWidget);
    await t.tap(find.byKey(ValueKey('app-copy-${'a' * 64}')));
    await t.pump();
    expect(clip, 'yandi bootstrap from-card X', reason: 'в буфер уходит готовая команда для списка');
    await t.tap(find.byKey(ValueKey('app-dismiss-${'a' * 64}')));
    await t.pumpAndSettle();
    expect(api.calls, contains('/api/admin/bootstrap/applications/dismiss'));
    expect(api.dismissed, ['a' * 16]);
    await t.pumpAndSettle(const Duration(seconds: 3));
    // английский: ни одной русской надписи
    await t.pumpWidget(panel(true));
    await t.pumpAndSettle();
    final cyr = RegExp(r'[А-Яа-яЁё]');
    final texts = [
      for (final w in find.byType(Text, skipOffstage: false).evaluate().map((e) => e.widget as Text)) if (w.data != null && cyr.hasMatch(w.data!)) w.data!,
      for (final w in find.byType(SelectableText, skipOffstage: false).evaluate().map((e) => e.widget as SelectableText)) if (w.data != null && cyr.hasMatch(w.data!)) w.data!,
    ];
    expect(texts, isEmpty, reason: 'русский в английском режиме');
    expect(find.text('Node 203.0.113.5:9000: answers'), findsOneWidget);
  });
}

class _BootstrapApi extends m1.FakeApi {
  final calls = <String>[];
  final dismissed = <String>[];
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    calls.add(path);
    if (path == '/api/admin/bootstrap/applications') {
      return {
        'applications': [
          {
            'id': 'a' * 64,
            'key': 'b' * 64,
            'addr': ['203.0.113.5:9000'],
            'mobile': ['203.0.113.5:8444'],
            'region': 'NL',
            'roles': ['entry', 'guest'],
            'received': 1,
            'status': 'verified',
            'probes': [
              {'addr': '203.0.113.5:9000', 'kind': 'node', 'result': 'ok'},
              {'addr': '203.0.113.5:8444', 'kind': 'mobile', 'result': 'fail'},
            ],
            'card': 'YANDI-NODE-1:x',
            'command': 'yandi bootstrap from-card X',
          },
        ],
      };
    }
    if (path == '/api/admin/bootstrap/pubkey') return {'unlocked': true, 'pubkey': 'b' * 64, 'pinned': true};
    if (path == '/api/admin/bootstrap/applications/dismiss') {
      dismissed.add('${body?['id']}');
      return {'status': 'ok'};
    }
    return super.request(path, body: body, method: method);
  }
}
