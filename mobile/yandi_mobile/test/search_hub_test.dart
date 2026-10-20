import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:yandi_mobile/l10n.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/screens/search_hub.dart';
import 'package:yandi_mobile/services/app_state.dart';

Contact person(String id, String name, String role, {bool online = false, int last = 0}) =>
    Contact(peerId: id.padRight(64, '0'), displayName: name, online: online, isManual: false, lastActivityMs: last, role: role);

void main() {
  final longAgo = DateTime.now().millisecondsSinceEpoch - const Duration(days: 40).inMilliseconds;

  AppState stateWith(List<Contact> cs) {
    final s = AppState();
    s.contacts = cs;
    return s;
  }

  Widget host(AppState s, Widget child) => ChangeNotifierProvider<AppState>.value(value: s, child: MaterialApp(home: Scaffold(body: child)));

  testWidgets('каталог делится по роли и ничего не прячет по времени: офлайн полгода назад остаётся', (t) async {
    final s = stateWith([
      person('a', 'Гость онлайн', 'guest', online: true),
      person('b', 'Гость давно', 'guest', last: longAgo),
      person('c', 'Мама', 'user', online: true),
      person('d', 'Папа давно', 'user', last: longAgo),
      person('e', 'Ждёт подтверждения', 'invited'),
      person('f', 'Владелец', 'owner'),
    ]);
    expect(visibleUsers(s, guests: true).map((c) => c.displayName), ['Гость онлайн', 'Гость давно', 'Ждёт подтверждения']);
    expect(visibleUsers(s, guests: false).map((c) => c.displayName), ['Мама', 'Владелец', 'Папа давно']);
    expect(visibleUsers(s).length, 6, reason: 'в сети и не в сети — все');
  });

  testWidgets('страница поиска: карточки «Гости», «Пользователи», «Группы» со счётчиками; список гостей со статусом', (t) async {
    final s = stateWith([person('a', 'Антон', 'guest', online: true), person('b', 'Борис', 'guest'), person('c', 'Мама', 'user', online: true)]);
    await t.pumpWidget(host(s, const SearchHub()));
    await t.pump();
    expect(find.byKey(const ValueKey('hub-guests')), findsOneWidget);
    expect(find.byKey(const ValueKey('hub-users')), findsOneWidget);
    expect(find.text('Гости'), findsOneWidget);
    expect(find.text('Пользователи'), findsOneWidget);
    expect(find.text('Группы'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('hub-guests')));
    await t.pumpAndSettle();
    expect(find.text('Антон'), findsOneWidget);
    expect(find.text('Борис'), findsOneWidget);
    expect(find.text('Мама'), findsNothing, reason: 'авторизованные — в своей карточке');
    expect(find.text('Онлайн'), findsOneWidget);
    expect(find.text('Не в сети'), findsOneWidget);
  });

  testWidgets('страница поиска по-английски', (t) async {
    Lang.forTests(AppLang.en);
    addTearDown(() => Lang.forTests(AppLang.ru));
    await t.pumpWidget(host(stateWith([person('a', 'Anton', 'guest')]), const SearchHub()));
    await t.pump();
    expect(find.text('Guests'), findsOneWidget);
    expect(find.text('Users'), findsOneWidget);
    expect(find.text('Groups'), findsOneWidget);
  });
}
