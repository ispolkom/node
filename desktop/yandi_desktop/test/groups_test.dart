import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/group_repository.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'package:yandi_desktop/preferences.dart';
import 'widget_test.dart' as m1;

final gid = 'a' * 32, other = 'b' * 32;

class GroupApi extends m1.FakeApi {
  String role = 'owner';
  bool hasKey = true, muted = false, rejectSend = false;
  Completer<Map<String, dynamic>>? pendingFeed, pendingSend;
  final members = <Map<String, dynamic>>[
    {'alias': 'self', 'name': 'Я', 'role': 'owner', 'has_key': true},
    {
      'alias': 'member',
      'name': '<b>Боб</b>',
      'role': 'member',
      'has_key': true,
    },
    {'alias': 'mod', 'name': 'Модератор', 'role': 'moderator', 'has_key': true},
    {'alias': 'admin', 'name': 'Админ', 'role': 'admin', 'has_key': true},
  ];
  final messages = <Map<String, dynamic>>[
    {
      'seq': 2,
      'ts': DateTime.now().millisecondsSinceEpoch,
      'from': 'member',
      'author': '<b>Боб</b>',
      'kind': 1,
      'body': {'t': '<script>текст</script>'},
    },
    {
      'seq': 1,
      'ts': 1,
      'kind': 1,
      'body': {'t': 'Древняя запись'},
    },
    {
      'seq': 3,
      'ts': DateTime.now().millisecondsSinceEpoch,
      'kind': 3,
      'body': {'k': 'SECRET-KEY', 'f': 'file'},
    },
  ];
  Map<String, dynamic> group(String id) => {
    'id': id,
    'name': id == gid ? 'Семья' : 'Работа',
    'role': role,
    'kind': 'invite',
    'topic': 'Семья',
    'about': 'Описание',
    'muted': muted,
  };
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (!path.startsWith('/api/groups') &&
        !path.startsWith('/api/mobile/groups')) {
      return super.request(path, body: body, method: method);
    }
    calls.add(path);
    if (body != null) bodies.add(Map.of(body));
    if (path == failPath) throw const ApiFailure('http', 400);
    if (path == '/api/groups')
      return {
        'groups': [group(gid), group(other)],
        'catalog': [
          {'id': 'c' * 32, 'name': 'Открытая', 'topic': 'Игры', 'kind': 'open'},
          {
            'id': 'd' * 32,
            'name': 'Закрытая',
            'topic': 'Игры',
            'kind': 'invite',
          },
        ],
        'gone': [
          {'id': 'e' * 32, 'name': 'Ушедшая'},
        ],
      };
    if (path == '/api/mobile/groups')
      return {
        'groups': [group(gid)],
      };
    if (path == '/api/groups/create') return {'id': gid};
    final id = path.split('/')[3];
    if (path.endsWith('/messages')) {
      if (id == gid && pendingFeed != null) return pendingFeed!.future;
      return {
        'group': group(id),
        'has_key': hasKey,
        'deny': <int>[],
        'messages': id == gid ? messages : <Map<String, dynamic>>[],
      };
    }
    if (path.endsWith('/info'))
      return {
        'group': group(id),
        'members': members,
        'requests': [
          {'id': 'req', 'name': 'Запрос'},
        ],
        'banned': <Map<String, dynamic>>[],
        'modlog': <Map<String, dynamic>>[],
        'deny': <int>[],
      };
    if (path.endsWith('/mod')) {
      if (body!['action'] == 'delete_entry')
        messages.removeWhere((m) => m['seq'] == body['seq']);
      if (body['action'] == 'kick')
        members.removeWhere((m) => m['alias'] == body['alias']);
    }
    if (path.endsWith('/invite-code'))
      return {'invite': 'YANDI-GROUP-1:$gid:CODE'};
    if (path.endsWith('/send')) {
      if (rejectSend) throw const ApiFailure('http', 400);
      if (pendingSend != null) return pendingSend!.future;
      messages.add({
        'seq': 4,
        'ts': DateTime.now().millisecondsSinceEpoch,
        'kind': 1,
        'mine': true,
        'body': {'t': body!['text']},
      });
    }
    return {'ok': true};
  }
}

Finder field(String label) => find.widgetWithText(TextField, label);
Future<void> open(
  WidgetTester t,
  GroupApi api, {
  bool conversation = false,
}) async {
  await m1.start(t, api);
  await m1.click(t, 'Войти');
  await m1.click(t, 'Группы');
  if (conversation) {
    await m1.click(t, 'Семья · ${russianStates[api.role]}');
    await m1.click(t, 'Настройки группы');
  }
}

void main() {
  test(
    'feed calendar cutoff, sort, visibility and secret projection',
    () async {
      final api = GroupApi();
      final cutoff = DateTime.utc(2025, 2, 28, 12).millisecondsSinceEpoch;
      api.messages.clear();
      api.messages.addAll([
        {
          'seq': 9,
          'ts': cutoff,
          'kind': 1,
          'body': {'t': 'Nine', 'k': 'SECRET'},
        },
        {'seq': 1, 'ts': cutoff - 1, 'kind': 1},
        {
          'seq': 2,
          'ts': cutoff,
          'kind': 1,
          'body': {'t': 'Two'},
        },
        {'seq': 3, 'ts': cutoff, 'kind': 7},
        {'seq': 4, 'ts': cutoff, 'kind': 1, 'deleted': true},
      ]);
      final result = await GroupRepository(
        api,
      ).feed(gid, now: DateTime.utc(2026, 2, 28, 12));
      expect((result['messages'] as List).map((m) => m['seq']), [2, 9]);
      expect(result.toString(), isNot(contains('SECRET')));
      await expectLater(
        GroupRepository(api).send(gid, 'я' * 4001),
        throwsA(isA<ApiFailure>()),
      );
      expect(
        () => GroupRepository(api).path('../id', 'info'),
        throwsA(isA<ApiFailure>()),
      );
      expect(GroupRepository.canTouch('admin', 'admin'), false);
      expect(GroupRepository.canTouch('moderator', 'moderator'), false);
      expect(GroupRepository.canTouch('owner', 'owner'), false);
    },
  );
  testWidgets(
    'list and creation validate required topic and open the created group',
    (t) async {
      final api = GroupApi();
      await open(t, api);
      expect(find.text('Работа · владелец'), findsOneWidget);
      await m1.click(t, 'Создать группу');
      await t.enterText(field('Название'), 'Новая');
      await m1.click(t, 'Подтвердить');
      expect(api.calls.contains('/api/groups/create'), false);
      await t.enterText(field('Тематика'), 'Друзья');
      await m1.click(t, 'Подтвердить');
      expect(
        api.bodies.any((b) => b['name'] == 'Новая' && b['kind'] == 'invite'),
        true,
      );
      await m1.click(t, 'Настройки группы');
      expect(find.text('Участники'), findsOneWidget);
      await m1.stop(t);
    },
  );
  testWidgets('catalog join, invite request, join code and forget confirm', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api);
    await m1.click(t, 'Список групп');
    await m1.click(t, 'Вступить');
    await t.enterText(field('Ваше имя'), 'Имя');
    await m1.click(t, 'Подтвердить');
    expect(api.calls.contains('/api/groups/${'c' * 32}/join'), true);
    await m1.click(t, 'Попросить приглашение');
    await t.enterText(field('Ваше имя'), 'Имя');
    await m1.click(t, 'Подтвердить');
    expect(api.calls.contains('/api/groups/${'d' * 32}/request'), true);
    await m1.click(t, 'Вступить по приглашению');
    await t.enterText(
      field('Приглашение YANDI-GROUP-1 или ID'),
      'YANDI-GROUP-1:$gid:CODE',
    );
    await t.enterText(field('Ваше имя'), 'Имя');
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.any((b) => b['code'] == 'CODE'), true);
    await m1.click(t, 'Мои группы');
    await m1.click(t, 'Забыть Ушедшая');
    await m1.click(t, 'Отмена');
    expect(api.calls.any((p) => p.endsWith('/forget')), false);
    await m1.stop(t);
  });
  testWidgets(
    'plain text, send failure preserves draft, single flight and Ctrl Enter',
    (t) async {
      final api = GroupApi();
      await open(t, api, conversation: true);
      expect(find.text('<script>текст</script>'), findsOneWidget);
      expect(find.text('Древняя запись'), findsNothing);
      expect(find.text('SECRET-KEY'), findsNothing);
      api.rejectSend = true;
      await t.enterText(field('Сообщение группе'), 'Черновик');
      await m1.click(t, 'Отправить группе');
      expect(
        t.widget<TextField>(field('Сообщение группе')).controller!.text,
        'Черновик',
      );
      expect(api.calls.where((p) => p.endsWith('/send')).length, 1);
      api.rejectSend = false;
      api.pendingSend = Completer();
      await t.ensureVisible(field('Сообщение группе'));
      await t.tap(field('Сообщение группе'));
      await t.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await t.sendKeyEvent(LogicalKeyboardKey.enter);
      await t.pump();
      await t.sendKeyEvent(LogicalKeyboardKey.enter);
      await t.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      expect(api.calls.where((p) => p.endsWith('/send')).length, 2);
      api.pendingSend!.complete({'seq': 5});
      await t.pumpAndSettle();
      expect(
        t.widget<TextField>(field('Сообщение группе')).controller!.text,
        '',
      );
      await m1.stop(t);
    },
  );
  testWidgets('drafts per group, full refresh removes deleted entries', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    await t.enterText(field('Сообщение группе'), 'Семейный черновик');
    await m1.click(t, 'Работа · владелец');
    expect(t.widget<TextField>(field('Сообщение группе')).controller!.text, '');
    await m1.click(t, 'Семья · владелец');
    expect(
      t.widget<TextField>(field('Сообщение группе')).controller!.text,
      'Семейный черновик',
    );
    api.messages.clear();
    await t.pump(const Duration(seconds: 4));
    await t.pumpAndSettle();
    expect(find.text('<script>текст</script>'), findsNothing);
    await m1.stop(t);
  });
  testWidgets('delete entry confirmation and mutation preflight lock', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    await m1.click(t, 'Удалить сообщение №2');
    await m1.click(t, 'Подтвердить');
    expect(find.text('<script>текст</script>'), findsNothing);
    await t.enterText(field('Сообщение группе'), 'Не отправлять');
    api.configured = true;
    api.unlocked = false;
    await t.ensureVisible(find.text('Отправить группе'));
    await t.tap(find.text('Отправить группе'));
    await t.pumpAndSettle();
    expect(api.calls.any((p) => p.endsWith('/send')), false);
    expect(field('Сообщение группе'), findsNothing);
    await m1.stop(t);
  });
  for (final role in ['member', 'moderator', 'admin', 'owner']) {
    testWidgets('participants rights $role and explicit kick confirmation', (
      t,
    ) async {
      final api = GroupApi()..role = role;
      await open(t, api, conversation: true);
      expect(
        find.text('Исключить'),
        findsNWidgets(
          {'member': 0, 'moderator': 1, 'admin': 2, 'owner': 3}[role]!,
        ),
      );
      expect(
        find.text('Назначить админом'),
        findsNWidgets(role == 'owner' ? 3 : 0),
      );
      expect(
        find.text('Создать приглашение'),
        role == 'member' ? findsNothing : findsOneWidget,
      );
      if (role == 'owner') {
        await m1.click(t, 'Исключить');
        await m1.click(t, 'Отмена');
        expect(api.calls.any((p) => p.endsWith('/mod')), false);
        await m1.click(t, 'Исключить');
        await m1.click(t, 'Подтвердить');
        expect(
          api.bodies.any(
            (b) => b['action'] == 'kick' && b['alias'] == 'member',
          ),
          true,
        );
      }
      await m1.stop(t);
    });
  }
  testWidgets('settings drafts survive polling, rules and code invitation', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    await m1.click(t, 'Изменить настройки группы');
    await t.enterText(field('Название'), 'Несохранённое');
    await t.pump(const Duration(seconds: 8));
    await t.pumpAndSettle();
    expect(
      t.widget<TextField>(field('Название')).controller!.text,
      'Несохранённое',
    );
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.any((b) => b['name'] == 'Несохранённое'), true);
    await m1.click(t, 'Правила группы');
    await t.tap(find.text('Запретить Ссылки'));
    await m1.click(t, 'Сохранить правила');
    expect(
      api.bodies.any(
        (b) => b['deny'] is List && (b['deny'] as List).contains(6),
      ),
      true,
    );
    await m1.click(t, 'Создать приглашение');
    expect(find.text('YANDI-GROUP-1:$gid:CODE'), findsOneWidget);
    await m1.stop(t);
  });
  testWidgets('ownership countdown and node delete StepUp', (t) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    await m1.click(t, 'Передать владение');
    expect(find.text('Подождите 3 с'), findsOneWidget);
    await t.pump(const Duration(seconds: 3));
    await t.pumpAndSettle();
    await m1.click(t, 'Подтвердить');
    expect(api.calls.any((p) => p.endsWith('/transfer')), true);
    await m1.click(t, 'Управление узла');
    expect(find.text('Сообщение группе'), findsNothing);
    await m1.click(t, 'Удалить группу Семья');
    await t.enterText(field('Пароль входа узла'), 'test-password');
    await m1.click(t, 'Отмена');
    expect(api.calls.any((p) => p.endsWith('/delete')), false);
    await m1.click(t, 'Удалить группу Семья');
    expect(
      t.widget<TextField>(field('Пароль входа узла')).controller!.text,
      '',
    );
    await t.enterText(field('Пароль входа узла'), 'test-password');
    await m1.click(t, 'Удалить навсегда');
    expect(api.bodies.any((b) => b['password'] == 'test-password'), true);
    await m1.stop(t);
  });
  testWidgets('late feed after switching cannot replace current group', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    api.pendingFeed = Completer();
    await t.pump(const Duration(seconds: 4));
    await t.pump();
    await t.ensureVisible(find.text('Работа · владелец'));
    await t.tap(find.text('Работа · владелец'));
    await t.pump();
    api.pendingFeed!.complete({
      'group': api.group(gid),
      'has_key': true,
      'deny': [],
      'messages': api.messages,
    });
    await t.pump(const Duration(milliseconds: 50));
    await t.pumpAndSettle();
    expect(find.text('<script>текст</script>'), findsNothing);
    expect(find.text('Работа'), findsOneWidget);
    await m1.stop(t);
  });
  testWidgets('remote lock closes dialog, drops invitation and draft', (
    t,
  ) async {
    final api = GroupApi();
    await open(t, api, conversation: true);
    await t.enterText(field('Сообщение группе'), 'Секрет');
    await m1.click(t, 'Создать приглашение');
    await m1.click(t, 'Изменить настройки группы');
    api.configured = true;
    api.unlocked = false;
    await t.pump(const Duration(seconds: 1));
    await t.pumpAndSettle();
    expect(field('Название'), findsNothing);
    expect(field('Сообщение группе'), findsNothing);
    expect(find.text('YANDI-GROUP-1:$gid:CODE'), findsNothing);
    await m1.stop(t);
  });
  testWidgets('missing key and muted group disable composer, narrow layout', (
    t,
  ) async {
    await t.binding.setSurfaceSize(const Size(400, 500));
    addTearDown(() => t.binding.setSurfaceSize(null));
    final api = GroupApi()
      ..hasKey = false
      ..muted = true;
    await open(t, api, conversation: true);
    expect(find.text('Ждём ключ группы.'), findsOneWidget);
    expect(t.widget<TextField>(field('Сообщение группе')).enabled, false);
    expect(t.takeException(), isNull);
    await m1.stop(t);
  });
}
