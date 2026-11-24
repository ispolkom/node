import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/shell_repository.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'widget_test.dart' as m1;

final alice = 'a' * 64, bob = 'b' * 64;

class FeatureApi extends m1.FakeApi {
  final methods = <String>[];
  final rows = <Map<String, dynamic>>[
    {'id': 'one', 'name': 'Алиса', 'short_id': 'a' * 16, 'online': true},
    {'id': 'two', 'name': 'Боб', 'short_id': 'b' * 16, 'online': false},
  ];
  String profileName = 'Мой профиль';
  bool rejectSend = false;
  Completer<Map<String, dynamic>>? historyPending, sendPending;
  List<Map<String, dynamic>> historyRows = [
    {
      'msg_id': 'new',
      'from': 'c' * 64,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': '<b>Текст</b>',
      'status': 'Sent',
    },
    {
      'msg_id': 'old',
      'from': alice,
      'timestamp': DateTime(2020).millisecondsSinceEpoch,
      'text': 'Старый текст',
    },
  ];
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path.startsWith('/api/profile/') && path != '/api/profile/me') {
      return {
        'profile': {
          'display_name': 'Алиса',
          'short_id': path.split('/').last,
          'online': true,
        },
      };
    }
    if (path == '/api/status') return {'node_id': 'c' * 64, 'short_id': 'abc'};
    if (path == '/api/contacts' ||
        path == '/api/peers/trusted' ||
        path == '/api/profile/me' ||
        path.startsWith('/api/chat/')) {
      calls.add(path);
      methods.add(method ?? (body == null ? 'GET' : 'POST'));
      if (body != null) bodies.add(Map.of(body));
      if (path == failPath) throw const FormatException('fixture failure');
      if (path == '/api/contacts') {
        if (method == 'DELETE') {
          rows.removeWhere((r) => r['id'] == body!['id']);
        } else if (body != null && body['id'] != null) {
          rows.firstWhere((r) => r['id'] == body['id'])['name'] = body['name'];
        } else if (body != null) {
          rows.add({'id': 'added', ...body, 'online': false});
        }
        return {'contacts': rows.map(Map<String, dynamic>.of).toList()};
      }
      if (path == '/api/peers/trusted') {
        if (body != null) return {'ok': true, 'id': alice};
        return {
          'card': 'YANDI-NODE-1:test',
          'peers': [
            {'id': alice},
            {'id': bob},
          ],
        };
      }
      if (path == '/api/profile/me') {
        if (body != null) profileName = body['display_name'] as String;
        return {
          'profile': {'display_name': profileName},
        };
      }
      if (path.startsWith('/api/chat/history/')) {
        if (historyPending != null && path.endsWith(alice))
          return historyPending!.future;
        return {'messages': historyRows.map(Map<String, dynamic>.of).toList()};
      }
      if (rejectSend) throw const ApiFailure('business', 400);
      if (sendPending != null) return sendPending!.future;
      return {'status': 'success'};
    }
    return super.request(path, body: body, method: method);
  }
}

Future<void> open(WidgetTester t, FeatureApi api, String page) async {
  await m1.start(t, api);
  await m1.click(t, 'Войти');
  await m1.click(t, page);
}

/// Страницы управления узлом открываются через админ-панель.
Future<void> openAdmin(WidgetTester t, FeatureApi api, String page) async {
  await m1.start(t, api);
  await m1.click(t, 'Войти');
  await m1.admin(t, page);
}

Finder field(String label) => find.widgetWithText(TextField, label);
void main() {
  test(
    'calendar year retention, millisecond timestamp and stable order',
    () async {
      final api = FeatureApi();
      final cutoff = DateTime.utc(2025, 10, 12, 12).millisecondsSinceEpoch;
      api.historyRows = [
        {'msg_id': 'z', 'timestamp': cutoff},
        {'msg_id': 'old', 'timestamp': cutoff - 1},
        {'msg_id': 'a', 'timestamp': cutoff},
      ];
      final rows = await ShellRepository(
        api,
      ).history(alice, now: DateTime.utc(2026, 10, 12, 12));
      expect(rows.map((r) => r['msg_id']), ['a', 'z']);
      await expectLater(
        ShellRepository(api).send('../peer', 'text'),
        throwsException,
      );
    },
  );
  testWidgets('contacts card import, rename and explicit remove confirmation', (
    t,
  ) async {
    final api = FeatureApi();
    await open(t, api, 'Чат');
    expect(find.text('Алиса · В сети'), findsOneWidget);
    expect(find.text('Боб · Не в сети'), findsOneWidget);
    expect(find.text('Контакты'), findsOneWidget, reason: 'отдельной вкладки «Контакты» нет, есть панель');
    await m1.click(t, 'Добавить');
    await t.enterText(field('Имя контакта'), 'Новый контакт');
    await t.enterText(field('Карточка YANDI-NODE-1'), 'YANDI-NODE-1:test');
    await m1.click(t, 'Добавить контакт');
    expect(api.bodies.any((b) => b['card'] == 'YANDI-NODE-1:test'), true);
    expect(find.text('Новый контакт · Не в сети'), findsOneWidget);
    await m1.click(t, 'Переименовать');
    await t.enterText(find.widgetWithText(TextField, 'Имя'), 'Алиса новая');
    await m1.click(t, 'Сохранить');
    expect(find.text('Алиса новая · В сети'), findsOneWidget);
    await m1.click(t, 'Удалить контакт');
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(api.methods.contains('DELETE'), false);
    await m1.click(t, 'Отмена');
    await m1.click(t, 'Удалить контакт');
    await m1.click(t, 'Удалить');
    expect(api.methods.contains('DELETE'), true);
    expect(find.text('Алиса новая · В сети'), findsNothing);
    await m1.stop(t);
  });
  testWidgets(
    'chat plain text, retention, isolated drafts and Enter single flight',
    (t) async {
      final api = FeatureApi();
      await open(t, api, 'Чат');
      expect(find.text('Выберите контакт для переписки.'), findsOneWidget);
      await m1.click(t, 'Открыть чат');
      expect(find.text('<b>Текст</b>'), findsOneWidget);
      expect(find.text('Старый текст'), findsNothing);
      await t.enterText(field('Сообщение'), 'черновик Алисы');
      final second = find.widgetWithText(TextButton, 'Открыть чат').at(1);
      await t.ensureVisible(second);
      await t.tap(second);
      await t.pumpAndSettle();
      expect(t.widget<TextField>(field('Сообщение')).controller!.text, isEmpty);
      await m1.click(t, 'Открыть чат');
      expect(
        t.widget<TextField>(field('Сообщение')).controller!.text,
        'черновик Алисы',
      );
      api.sendPending = Completer<Map<String, dynamic>>();
      await t.ensureVisible(field('Сообщение'));
      await t.tap(field('Сообщение'));
      await t.sendKeyEvent(LogicalKeyboardKey.enter);
      await t.pump();
      await t.sendKeyEvent(LogicalKeyboardKey.enter);
      await t.pump();
      expect(api.calls.where((c) => c.startsWith('/api/chat/send/')).length, 1);
      api.sendPending!.complete({'status': 'success'});
      await t.pumpAndSettle();
      expect(t.widget<TextField>(field('Сообщение')).controller!.text, isEmpty);
      await m1.stop(t);
    },
  );
  testWidgets('late history cannot populate another peer or locked screen', (
    t,
  ) async {
    final api = FeatureApi()
      ..configured = true
      ..unlocked = true;
    await open(t, api, 'Чат');
    api.historyPending = Completer<Map<String, dynamic>>();
    final first = find.widgetWithText(TextButton, 'Открыть чат').first;
    await t.ensureVisible(first);
    await t.tap(first);
    await t.pump();
    final second = find.widgetWithText(TextButton, 'Открыть чат').at(1);
    await t.ensureVisible(second);
    await t.tap(second);
    await t.pump();
    api.historyPending!.complete({
      'messages': [
        {
          'msg_id': 'late',
          'timestamp': DateTime.now().millisecondsSinceEpoch,
          'text': 'Чужой поздний ответ',
        },
      ],
    });
    await t.pump(const Duration(milliseconds: 30));
    await t.pumpAndSettle();
    expect(find.byTooltip('Файлы чата'), findsOneWidget);   // чат открыт (заголовка с именем в полосе иконок больше нет)
    expect(find.text('Чужой поздний ответ'), findsNothing);
    await t.enterText(field('Сообщение'), 'секретный черновик');
    api.unlocked = false;
    await t.pump(const Duration(seconds: 1));
    await t.pumpAndSettle();
    expect(find.text('секретный черновик'), findsNothing);
    expect(find.text('<b>Текст</b>'), findsNothing);
    expect(find.textContaining('Закрытая среда заперта'), findsOneWidget);
    await m1.stop(t);
  });
  testWidgets('settings profile PUT, draft preserved and password contract', (
    t,
  ) async {
    final api = FeatureApi();
    await open(t, api, 'Настройки');
    expect(
      t.widget<TextField>(field('Имя профиля')).controller!.text,
      'Мой профиль',
    );
    await t.enterText(field('Имя профиля'), 'Новое имя');
    await t.pump(const Duration(seconds: 10));
    await t.pumpAndSettle();
    expect(
      t.widget<TextField>(field('Имя профиля')).controller!.text,
      'Новое имя',
    );
    await m1.click(t, 'Сохранить профиль');
    expect(api.profileName, 'Новое имя');
    expect(api.methods.contains('PUT'), true);
    // формы смены пароля входа на этой странице больше нет: браузерного входа нет
    expect(find.text('Сменить пароль входа'), findsNothing);
    await m1.stop(t);
  });
  testWidgets(
    'Shift Enter does not send; failed send preserves draft without retry',
    (t) async {
      final api = FeatureApi()..rejectSend = true;
      await open(t, api, 'Чат');
      await m1.click(t, 'Открыть чат');
      await t.enterText(field('Сообщение'), 'Сохранить черновик');
      await t.ensureVisible(field('Сообщение'));
      await t.tap(field('Сообщение'));
      await t.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await t.sendKeyEvent(LogicalKeyboardKey.enter);
      await t.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await t.pump();
      expect(api.calls.where((c) => c.startsWith('/api/chat/send/')), isEmpty);
      await m1.click(t, 'Отправить');
      expect(
        t.widget<TextField>(field('Сообщение')).controller!.text,
        'Сохранить черновик',
      );
      expect(find.textContaining('Демон отклонил действие'), findsOneWidget);
      await t.pump(const Duration(seconds: 10));
      await t.pumpAndSettle();
      expect(api.calls.where((c) => c.startsWith('/api/chat/send/')).length, 1);
      await m1.stop(t);
    },
  );
  testWidgets('remote lock dismisses rename dialog and hides its text', (
    t,
  ) async {
    final api = FeatureApi()
      ..configured = true
      ..unlocked = true;
    await open(t, api, 'Чат');
    await m1.click(t, 'Переименовать');
    await t.enterText(find.widgetWithText(TextField, 'Имя'), 'Секретное имя');
    api.unlocked = false;
    await t.pump(const Duration(seconds: 1));
    await t.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(find.text('Секретное имя'), findsNothing);
    expect(find.textContaining('Закрытая среда заперта'), findsOneWidget);
    await m1.stop(t);
  });
  testWidgets('small window contacts and chat remain usable', (t) async {
    await t.binding.setSurfaceSize(const Size(400, 500));
    addTearDown(() => t.binding.setSurfaceSize(null));
    await open(t, FeatureApi(), 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.ensureVisible(field('Сообщение'));
    expect(t.takeException(), isNull);
    await m1.stop(t);
  });
  testWidgets('не добавленные в контакты видны только в поиске и добавляются словом', (t) async {
    final api = FeatureApi();
    api.rows.removeWhere((r) => r['id'] == 'two'); // Боб известен узлу, но не в контактах
    await open(t, api, 'Чат');
    expect(find.text('Не в контактах'), findsNothing);
    await t.enterText(field('Поиск по имени или ID'), 'bbbb');
    await t.pump();
    expect(find.text('Не в контактах'), findsOneWidget);
    await m1.word(t, 'Добавить в контакты');
    expect(api.bodies.any((b) => b['short_id'] == 'b' * 16), true);
    await m1.stop(t);
  });
}
