import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/search_page.dart';
import 'package:yandi_desktop/transport/api.dart';

class _Api implements Api {
  final calls = <String>[];
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    calls.add(path);
    if (path == '/api/mobile/directory') {
      return {
        'devices': [
          {'peer_id': 'a' * 64, 'name': 'Телефон Ани', 'online': true, 'role': 'user', 'saved': true, 'status': 'ok'},
          {'peer_id': 'b' * 64, 'name': 'Планшет', 'online': false, 'role': 'user', 'saved': false, 'status': 'ok'},
          {'peer_id': 'c' * 64, 'name': 'Гость Антон', 'online': true, 'role': 'guest', 'saved': false, 'status': 'ok'},
          {'peer_id': 'd' * 64, 'name': 'Гость Борис', 'online': false, 'role': 'guest', 'saved': false, 'status': 'ok'},
          {'peer_id': 'e' * 64, 'name': 'Гость Вера', 'online': false, 'role': 'invited', 'saved': false, 'status': 'ok'},
        ],
      };
    }
    if (path == '/api/groups') {
      return {
        'catalog': [
          {'id': 'g1', 'name': 'Болталка', 'topic': 'Общение', 'kind': 'open', 'members': 7, 'owner_present': true},
          {'id': 'g2', 'name': 'Закрытая', 'topic': 'Свои', 'kind': 'invite', 'members': 3, 'owner_present': true},
        ],
      };
    }
    return {};
  }

  @override
  void cancel() {}
}

void main() {
  testWidgets('три карточки (гости, пользователи, группы) с количеством; пользователи со статусом, группы с видом и числом участников', (t) async {
    final api = _Api();
    await t.pumpWidget(
      PreferencesScope(
        preferences: AppPreferences(),
        child: MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: SearchFeature(api: api, isActive: () => true, onFailure: (_) {}),
            ),
          ),
        ),
      ),
    );
    await t.pump();
    await t.pump();
    expect(find.text('Гости'), findsOneWidget);
    expect(find.text('Пользователи'), findsOneWidget);
    expect(find.text('Группы'), findsOneWidget);
    expect(find.text('3'), findsOneWidget, reason: 'гостей трое (приглашённый, не подтверждённый, считается гостем)');
    expect(find.text('2'), findsNWidgets(2), reason: 'пользователей и групп по два');

    await t.tap(find.text('Гости'));
    await t.pump();
    expect(find.text('Гость Антон'), findsOneWidget);
    expect(find.text('Гость Вера'), findsOneWidget);
    expect(find.text('Телефон Ани'), findsNothing, reason: 'авторизованные — в своей карточке');
    expect(find.text('Онлайн'), findsOneWidget);
    expect(find.text('Не в сети'), findsNWidgets(2));
    await t.tap(find.text('Назад'));
    await t.pump();

    await t.tap(find.text('Пользователи'));
    await t.pump();
    expect(find.text('Телефон Ани'), findsOneWidget);
    expect(find.text('в контактах'), findsOneWidget, reason: 'сохранённый контакт остаётся в общем списке с отметкой');
    expect(find.text('Планшет'), findsOneWidget);
    expect(find.text('Онлайн'), findsOneWidget);
    expect(find.text('Не в сети'), findsOneWidget);

    await t.tap(find.text('Назад'));
    await t.pump();
    await t.tap(find.text('Группы'));
    await t.pump();
    expect(find.text('Болталка'), findsOneWidget);
    expect(find.text('Открытые группы (1)'), findsOneWidget);
    expect(find.text('Группы по приглашению (1)'), findsOneWidget);
    expect(find.text('Открытая'), findsOneWidget);
    expect(find.text('По приглашению'), findsOneWidget);
    expect(find.text('7 уч.'), findsOneWidget);
    await t.pumpWidget(const SizedBox());
  });
  englishTest();
}

void englishTest() {
  testWidgets('карточки поиска по-английски', (t) async {
    final prefs = AppPreferences()..english = true;
    await t.pumpWidget(PreferencesScope(
      preferences: prefs,
      child: MaterialApp(home: Scaffold(body: SingleChildScrollView(child: SearchFeature(api: _Api(), isActive: () => true, onFailure: (_) {})))),
    ));
    await t.pump();
    await t.pump();
    expect(find.text('Guests'), findsOneWidget);
    expect(find.text('Users'), findsOneWidget);
    expect(find.text('Groups'), findsOneWidget);
    await t.pumpWidget(const SizedBox());
  });
}
