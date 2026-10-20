import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:yandi_mobile/l10n.dart';
import 'package:yandi_mobile/models/guest_node.dart';
import 'package:yandi_mobile/models/trusted_node.dart';
import 'package:yandi_mobile/screens/guest_screen.dart';
import 'package:yandi_mobile/services/guest_pair.dart';
import 'package:yandi_mobile/widgets/locked_notice.dart';

final fp = 'ab' * 32;

void main() {
  test('список узлов для гостей: верные записи берутся, мусор отбрасывается', () {
    final l = GuestNode.parseList(jsonEncode({
      'nodes': [
        {'name': 'Узел 1', 'host': '203.0.113.7', 'port': 8444, 'fingerprint': fp},
        {'name': 'node.example', 'host': 'node.example', 'port': 443, 'fingerprint': fp.toUpperCase()},
        {'name': '', 'host': 'x.example', 'port': 1, 'fingerprint': fp},
        {'name': 'плохой порт', 'host': 'x.example', 'port': 70000, 'fingerprint': fp},
        {'name': 'плохой отпечаток', 'host': 'x.example', 'port': 443, 'fingerprint': 'zz'},
        {'name': 'плохой адрес', 'host': 'a b/c', 'port': 443, 'fingerprint': fp},
        'строка',
      ]
    }));
    expect(l.map((n) => n.name).toList(), ['Узел 1', 'node.example']);
    expect(l[1].fingerprint, fp, reason: 'отпечаток приводится к нижнему регистру');
    expect(GuestNode.parseList('не json'), isEmpty);
    expect(GuestNode.parseList({'nodes': 5}), isEmpty);
    expect(GuestNode.parseList(jsonEncode({'nodes': []})), isEmpty);
  });

  test('вход гостем: токен роли guest, узел запоминается с закреплённым отпечатком; отказы понятны', () async {
    final seen = <String>[];
    final client = MockClient((req) async {
      seen.add('${req.method} ${req.url.path} ${req.headers['Authorization'] ?? ''}');
      if (req.url.path == '/mobile/pair/guest') return http.Response(jsonEncode({'token': 'tok123', 'role': 'guest'}), 200);
      return http.Response(jsonEncode({'node_id': 'n' * 64, 'name': 'My node'}), 200);
    });
    final node = await guestPair(GuestNode('Узел', '203.0.113.7', 8444, fp), client: client, deviceName: 'Телефон ab12');
    expect(node.token, 'tok123');
    expect(node.id, 'n' * 64);
    expect((node.host, node.port, node.fingerprint), ('203.0.113.7', 8444, fp));
    expect(seen, ['POST /mobile/pair/guest ', 'GET /mobile/info Bearer tok123']);
    // не guest в ответе — отказ
    final wrong = MockClient((req) async => http.Response(jsonEncode({'token': 't', 'role': 'owner'}), 200));
    expect(() => guestPair(GuestNode('Узел', 'h.example', 443, fp), client: wrong, deviceName: 'x'), throwsA(isA<GuestPairError>()));
    // узел не принимает гостей / нет места / сеть
    for (final (code, part) in [(403, 'не принимает гостей'), (503, 'нет места'), (429, 'Слишком много')]) {
      final c = MockClient((req) async => http.Response('{}', code));
      await expectLater(guestPair(GuestNode('Узел', 'h.example', 443, fp), client: c, deviceName: 'x'), throwsA(predicate((e) => '$e'.contains(part))));
    }
    final broken = MockClient((req) async => throw http.ClientException('нет сети'));
    await expectLater(guestPair(GuestNode('Узел', 'h.example', 443, fp), client: broken, deviceName: 'x'), throwsA(predicate((e) => '$e'.contains('Не удалось связаться'))));
  });

  testWidgets('вкладка гостя: предупреждение на месте, пустой список — подсказка; одна кнопка; ни адреса, ни региона на экране; ошибка видна', (t) async {
    await t.pumpWidget(const MaterialApp(home: Scaffold(body: GuestPanel(nodes: []))));
    expect(find.byKey(const ValueKey('guest-intro')), findsOneWidget);
    expect(find.byKey(const ValueKey('guest-limits')), findsOneWidget);
    expect(find.byKey(const ValueKey('guest-empty')), findsOneWidget);
    var entered = 0;
    var fail = true;
    await t.pumpWidget(MaterialApp(
      home: Scaffold(
        body: GuestPanel(
          key: const ValueKey('with-nodes'),
          nodes: [GuestNode('Узел 1', '203.0.113.7', 8444, fp)],
          enter: (n) async {
            entered++;
            if (fail) throw const GuestPairError('На этом узле сейчас нет места для гостей. Попробуйте другой узел или позже.');
            return TrustedNode(id: 'n', name: n.name, host: n.host, port: n.port, fingerprint: n.fingerprint, token: 't', addedAt: 1);
          },
          onEntered: (_) {},
        ),
      ),
    ));
    // человеку не показывают ни адрес, ни имя узла
    expect(find.textContaining('203.0.113.7'), findsNothing);
    expect(find.textContaining('Узел 1'), findsNothing);
    await t.tap(find.byKey(const ValueKey('guest-enter')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(entered, 1);
    expect(find.byKey(const ValueKey('guest-error')), findsOneWidget);
    fail = false;
    await t.tap(find.byKey(const ValueKey('guest-enter')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(entered, 2);
    expect(find.byKey(const ValueKey('guest-error')), findsNothing);
  });

  testWidgets('вход гостем пробует узлы по очереди, пока один не примет', (t) async {
    final tried = <String>[];
    await t.pumpWidget(MaterialApp(
      home: Scaffold(
        body: GuestPanel(
          nodes: [GuestNode('A', '203.0.113.7', 8444, fp), GuestNode('B', '203.0.113.8', 8444, fp), GuestNode('C', '203.0.113.9', 8444, fp)],
          enter: (n) async {
            tried.add(n.host);
            if (n.host != '203.0.113.9') throw const GuestPairError('нет места');
            return TrustedNode(id: 'n', name: n.name, host: n.host, port: n.port, fingerprint: n.fingerprint, token: 't', addedAt: 1);
          },
          onEntered: (_) {},
        ),
      ),
    ));
    await t.tap(find.byKey(const ValueKey('guest-enter')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(tried.last, '203.0.113.9');
    expect(tried.toSet().length, tried.length);   // каждый узел не больше одного раза
    expect(find.byKey(const ValueKey('guest-error')), findsNothing);
  });

  testWidgets('вкладка гостя по-английски: те же элементы, надписи на английском', (t) async {
    Lang.forTests(AppLang.en);
    addTearDown(() => Lang.forTests(AppLang.ru));
    await t.pumpWidget(MaterialApp(home: Scaffold(body: GuestPanel(nodes: [GuestNode('Node 1', '203.0.113.7', 8444, fp)]))));
    expect(find.text('Sign in as a guest'), findsOneWidget);
    expect(find.textContaining('If you have no code or link'), findsOneWidget);
    expect(find.text('Attention'), findsOneWidget);
    expect(find.textContaining('Node 1'), findsNothing);
  });
  lockedNoticeTests();
}

void lockedNoticeTests() {
  testWidgets('закрытая гостю возможность: жёлтое окно с пояснением, по-русски и по-английски', (t) async {
    Future<void> open(WidgetTester t) async {
      await t.pumpWidget(MaterialApp(home: Builder(builder: (c) => Scaffold(body: Center(child: ElevatedButton(onPressed: () => showLockedNotice(c, what: tr('Видеозвонки')), child: const Text('go')))))));
      await t.tap(find.text('go'));
      await t.pumpAndSettle();
    }
    await open(t);
    expect(find.text('Доступно после полной авторизации'), findsOneWidget);
    expect(find.byKey(const ValueKey('locked-what')), findsOneWidget);
    expect(find.byKey(const ValueKey('locked-request')), findsOneWidget, reason: 'можно сразу запросить статус');
    Lang.forTests(AppLang.en);
    addTearDown(() => Lang.forTests(AppLang.ru));
    await t.pumpWidget(const SizedBox());
    await open(t);
    expect(find.text('Available after full authorisation'), findsOneWidget);
    expect(find.text('Video calls'), findsOneWidget);
    expect(find.text('Got it'), findsOneWidget);
  });

  testWidgets('закрытая вкладка целиком: жёлтая рамка вместо содержимого, нажатие открывает пояснение', (t) async {
    await t.pumpWidget(const MaterialApp(home: Scaffold(body: LockedPanel(title: 'Группы'))));
    expect(find.text('Доступно после полной авторизации'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('locked-panel')));
    await t.pumpAndSettle();
    expect(find.byKey(const ValueKey('locked-body')), findsOneWidget);
  });
}
