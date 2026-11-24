import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/dashboard.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/theme.dart';

Widget host(Widget child, {double width = 1000}) => MaterialApp(
  theme: Icq.theme,
  home: PreferencesScope(
    preferences: AppPreferences(),
    child: Scaffold(
      body: Center(
        child: SizedBox(width: width, child: SingleChildScrollView(child: child)),
      ),
    ),
  ),
);

final status = <String, dynamic>{
  'status': 'online',
  'short_id': 'abc',
  'node_id': 'f' * 64,
  'role': 'Border',
  'is_local': true,
  'external_ip': '203.0.113.7',
  'active_proxies': ['http://secret-user:secret-pass@10.0.0.1:3128', 'socks5://10.0.0.2:1080'],
};

void main() {
  testWidgets('статус: строки одной ширины и высоты, значения справа, Да/Нет, прокси не показываются', (t) async {
    await t.pumpWidget(host(NodeStatusView(status: status)));
    expect(find.text('Да'), findsWidgets);
    expect(find.textContaining('secret'), findsNothing, reason: 'сами прокси не выводятся');
    expect(find.textContaining('socks5'), findsNothing);
    final rows = find.byWidgetPredicate(
      (w) => w is Container && w.constraints?.maxHeight == 40,
    );
    expect(rows, findsNWidgets(NodeStatusView.rows.length));
    final sizes = [for (final e in t.widgetList(rows)) t.getSize(find.byWidget(e))];
    expect(sizes.map((s) => s.height).toSet().length, 1);
    expect(sizes.map((s) => s.width).toSet().length, 1);
    // подпись слева, значение прижато к правому краю белого поля
    final row = t.getRect(rows.first);
    final value = t.getRect(find.text('abc'));
    expect(value.right, closeTo(row.right - 12 - 1, 2));
    expect(t.getRect(find.text('Short ID')).left, lessThan(row.left + 20));
    // длинный ID узла умещается в строку
    expect(t.getRect(find.text('f' * 64)).right, lessThanOrEqualTo(row.right));
  });

  testWidgets('активные прокси: работают из добавленных, заперто — прочерк', (t) async {
    expect(NodeStatusView.upstream({'upstream_added': 3, 'upstream_working': 2}), '2 из 3');
    expect(NodeStatusView.upstream({'upstream_added': 0, 'upstream_working': 0}), '0 из 0');
    expect(NodeStatusView.upstream({'upstream_added': null, 'upstream_working': null}), '—');
    expect(NodeStatusView.upstream({}), '—');
    await t.pumpWidget(host(NodeStatusView(status: {...status, 'upstream_added': 2, 'upstream_working': 2})));
    expect(find.text('2 из 2'), findsOneWidget);
  });
  testWidgets('логические значения и «активные прокси» — только Да или Нет', (t) async {
    expect(NodeStatusView.show('is_local', false), 'Нет');
    expect(NodeStatusView.show('is_local', true), 'Да');
    expect(NodeStatusView.show('active_proxies', 0), 'Нет');
    expect(NodeStatusView.show('active_proxies', 3), 'Да');
    expect(NodeStatusView.show('active_proxies', const []), 'Нет');
    expect(NodeStatusView.show('active_proxies', const ['x']), 'Да');
    expect(NodeStatusView.show('active_proxies', null), 'Нет');
    expect(NodeStatusView.show('role', null), '—');
  });

  testWidgets('обнаруженные узлы: карточки одного размера на всю ширину блока, высота одна', (t) async {
    final nodes = [
      {'hostname': 'короткий', 'short_id': 'a1', 'connected': true},
      {'hostname': 'очень-длинное-имя-узла-которое-не-должно-растягивать-карточку', 'short_id': 'b2', 'addresses': '1.1.1.1:9000, 2.2.2.2:9000, 3.3.3.3:9000, 4.4.4.4:9000', 'role': 'Border'},
      {'short_id': 'c3'},
      {'short_id': 'd4', 'connected': false},
    ];
    await t.binding.setSurfaceSize(const Size(1200, 900));
    addTearDown(() => t.binding.setSurfaceSize(null));
    await t.pumpWidget(host(DashboardView(status: null, speed: null, nodes: nodes, controls: const SizedBox())));
    final cards = find.byWidgetPredicate((w) => w is SizedBox && w.height == 196);
    expect(cards, findsNWidgets(4));
    final sizes = [for (final e in t.widgetList(cards)) t.getSize(find.byWidget(e))];
    expect(sizes.map((s) => s.width).toSet().length, 1);
    expect(sizes.map((s) => s.height).toSet().length, 1);
    // три колонки заполняют ширину блока (панель 1000 минус поля)
    final first = t.getRect(cards.at(0)), third = t.getRect(cards.at(2));
    expect(first.height, 196);
    expect(third.top, first.top);
  });

  testWidgets('кнопки узла: зелёная и красная', (t) async {
    await t.pumpWidget(host(Row(children: [
      SolidButton('Запустить узел', onPressed: () {}),
      SolidButton('Остановить узел', tone: SolidTone.danger, onPressed: () {}),
    ])));
    Color bg(String label) {
      final b = t.widget<ElevatedButton>(find.widgetWithText(ElevatedButton, label));
      return b.style!.backgroundColor!.resolve({})!;
    }
    expect(bg('Запустить узел'), Icq.header);
    expect(bg('Остановить узел'), const Color(0xffb3261e));
  });
}
