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
    if (path == '/api/mobile/other-devices') {
      return {
        'devices': [
          {'peer_id': 'a' * 64, 'name': 'Телефон Ани', 'online': true},
          {'peer_id': 'b' * 64, 'name': 'Планшет', 'online': false},
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
  testWidgets('две карточки с количеством; пользователи со статусом, группы с видом и числом участников', (t) async {
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
    expect(find.text('Пользователи'), findsOneWidget);
    expect(find.text('Группы'), findsOneWidget);
    expect(find.text('2'), findsNWidgets(2), reason: 'и пользователей, и групп по два');

    await t.tap(find.text('Пользователи'));
    await t.pump();
    expect(find.text('Телефон Ани'), findsOneWidget);
    expect(find.text('Онлайн'), findsOneWidget);
    expect(find.text('Не в сети'), findsOneWidget);

    await t.tap(find.text('Назад'));
    await t.pump();
    await t.tap(find.text('Группы'));
    await t.pump();
    expect(find.text('Болталка'), findsOneWidget);
    expect(find.text('Открытая'), findsOneWidget);
    expect(find.text('По приглашению'), findsOneWidget);
    expect(find.text('7 уч.'), findsOneWidget);
    await t.pumpWidget(const SizedBox());
  });
}
