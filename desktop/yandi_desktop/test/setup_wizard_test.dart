import 'package:flutter/material.dart';
import '../tool/design_fixture.dart';
import 'package:yandi_desktop/app.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/setup_wizard.dart';
import 'package:yandi_desktop/transport/api.dart';

import 'widget_test.dart' show FakeApi, start, click, stop;

const seed12 = 'legal winner thank year wave sausage worth useful legal winner thank yellow';
final seed24 = '${List.filled(23, 'abandon').join(' ')} art';
final seed18 = '${List.filled(17, 'abandon').join(' ')} agent';

/// Узел для мастера: отдаёт сид по запросу, ключи создаёт один раз.
class WizardApi extends FakeApi {
  final asked = <int>[];
  ApiFailure? restoreFails;
  int? minPassword = 6;
  String inspectNodeId = '0123456789abcdef';
  WizardApi() {
    setup = true;
    configured = false;
    unlocked = false;
  }
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/auth/seed/new') {
      final n = (body?['words'] ?? 24) as int;
      asked.add(n);
      calls.add(path);
      return {'words': (n == 24 ? seed24 : n == 18 ? seed18 : seed12).split(' '), 'count': n, if (minPassword != null) 'min_password': minPassword};
    }
    if (path == '/api/auth/restore/inspect') {
      calls.add(path);
      bodies.add(Map.of(body!));
      return {'counter': 3, 'node_id': inspectNodeId, 'text': 'Копия №3 от 2026-10-17 09:05 UTC; узел abc'};
    }
    if (path == '/api/auth/seed/preview') {
      calls.add(path);
      bodies.add(Map.of(body!));
      if ((body['phrase'] as String).startsWith('bad ')) throw const ApiFailure('http', 400);
      return {'node_id': '0123456789abcdef', 'words': 12, 'control_word': (body['phrase'] as String).contains('\u001f')};
    }
    if (path == '/api/auth/restore' && restoreFails != null) {
      calls.add(path);
      throw restoreFails!;
    }
    final r = await super.request(path, body: body, method: method);
    if (path == '/api/auth/setup' || path == '/api/auth/restore') setup = false;
    return r;
  }
}

Future<void> toTypeStep(WidgetTester t, {List<String>? phrase}) async {
  final words = (phrase ?? seed12.split(' '));
  await click(t, 'Сохранил слова');
  for (final w in words) {
    final chip = find.widgetWithText(ElevatedButton, w);
    await t.ensureVisible(chip.first);
    await t.tap(chip.first);
    await t.pump();
  }
  await click(t, 'ОК');
}

/// Слова, которые мастер просит ввести, берутся из подписей полей («Слово №5»).
Future<void> typeAsked(WidgetTester t, List<String> words, {bool wrong = false}) async {
  final fields = find.byType(TextField);
  for (var i = 0; i < 3; i++) {
    final label = t.widget<TextField>(fields.at(i)).decoration!.labelText!;
    final n = int.parse(label.split('№').last);
    await t.enterText(fields.at(i), wrong ? 'zoo' : words[n - 1]);
  }
}

/// Слова введены верно: шаг контрольного слова пропущен (без слова), открыт шаг пароля.
Future<void> toPasswordStep(WidgetTester t, List<String> words) async {
  await toTypeStep(t, phrase: words);
  await typeAsked(t, words);
  await click(t, 'ОК');
  await click(t, 'Без контрольного слова');
}

void main() {
  testWidgets('wizard shows 24 words first, nothing is created until the password', (t) async {
    final api = WizardApi();
    await start(t, api);
    expect(find.text('Шаг 1 из 5'), findsOneWidget);
    expect(find.text('Выберите степень сложности сид-фразы'), findsOneWidget);
    expect(api.asked, [24]);
    expect(find.textContaining('art'), findsWidgets);
    expect(api.calls.contains('/api/auth/setup'), false);
    await stop(t);
  });

  testWidgets('12 or 18 words on request, a new phrase each time', (t) async {
    final api = WizardApi();
    await start(t, api);
    await t.tap(find.byType(DropdownButton<int>));
    await t.pumpAndSettle();
    await t.tap(find.text('12 слов (128 бит)').last);
    await t.pumpAndSettle();
    expect(api.asked, [24, 12]);
    expect(find.textContaining('yellow'), findsOneWidget);
    await t.tap(find.byType(DropdownButton<int>));
    await t.pumpAndSettle();
    await t.tap(find.text('18 слов (192 бита)').last);
    await t.pumpAndSettle();
    expect(api.asked, [24, 12, 18]);
    expect(find.textContaining('agent'), findsOneWidget);
    await stop(t);
  });

  testWidgets('wrong click order is refused and asks to look again', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: ['art', ...List.filled(23, 'abandon')]);
    expect(find.text('Порядок не совпал. Вернитесь и сверьте слова с записью.'), findsOneWidget);
    expect(find.text('Шаг 2 из 5'), findsOneWidget);
    await click(t, 'Показать слова снова');
    expect(find.text('Шаг 1 из 5'), findsOneWidget);
    expect(find.textContaining('art'), findsWidgets, reason: 'тот же сид, не новый');
    expect(api.asked, [24]);
    await stop(t);
  });

  testWidgets('three words by number are checked, then the control word step, then the password', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: seed24.split(' '));
    expect(find.text('Шаг 3 из 5'), findsOneWidget);
    expect(find.textContaining('abandon'), findsNothing, reason: 'слова на экране скрыты');
    await typeAsked(t, seed24.split(' '), wrong: true);
    await click(t, 'ОК');
    expect(find.text('Эти слова не совпали с записью. Проверьте запись.'), findsOneWidget);
    await typeAsked(t, seed24.split(' '));
    await click(t, 'ОК');
    expect(find.text('Шаг 4 из 5'), findsOneWidget);
    await click(t, 'Без контрольного слова');
    expect(find.text('Шаг 5 из 5'), findsOneWidget);
    // пароли: разные и короткий не уходят на узел
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'other password');
    await click(t, 'Создать узел');
    expect(find.text('Пароли не совпадают'), findsOneWidget);
    await t.enterText(find.byType(TextField).at(0), '123');
    await t.enterText(find.byType(TextField).at(1), '123');
    await click(t, 'Создать узел');
    expect(find.text('Пароль — не короче 6 символов'), findsOneWidget, reason: 'минимум — тот, что назвал узел (тестовый: 6)');
    expect(api.calls.contains('/api/auth/setup'), false);
    for (final f in t.widgetList<TextField>(find.byType(TextField))) {
      expect(f.obscureText && f.autocorrect == false && !f.enableSuggestions, true);
    }
    await stop(t);
  });

  testWidgets('the control word is optional, one word of 8-64 characters, typed twice, and a weak one is warned about', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: seed24.split(' '));
    await typeAsked(t, seed24.split(' '));
    await click(t, 'ОК');
    expect(find.text('Шаг 4 из 5'), findsOneWidget);
    final fields = find.byType(TextField);
    for (final f in t.widgetList<TextField>(fields)) {
      expect(f.obscureText, true, reason: 'слово не показывается');
    }
    await t.enterText(fields.at(0), 'два слова');
    await t.enterText(fields.at(1), 'два слова');
    await click(t, 'Далее');
    expect(find.text('Контрольное слово — одно слово, без пробелов'), findsOneWidget);
    await t.enterText(fields.at(0), 'коротко');
    await t.enterText(fields.at(1), 'коротко');
    await click(t, 'Далее');
    expect(find.text('Контрольное слово не короче 8 знаков'), findsOneWidget);
    await t.enterText(fields.at(0), '12345678');
    await t.pump();
    expect(find.text('Слово слабое: его легко угадать. Лучше 10 и больше случайных знаков.'), findsOneWidget);
    await t.enterText(fields.at(0), 'Воробей-2026');
    await t.enterText(fields.at(1), 'Воробей-2027');
    await click(t, 'Далее');
    expect(find.text('Слова не совпали'), findsOneWidget);
    expect(find.text('Шаг 4 из 5'), findsOneWidget);
    await t.enterText(fields.at(1), 'Воробей-2026');
    await click(t, 'Далее');
    expect(find.text('Шаг 5 из 5'), findsOneWidget);
    await stop(t);
  });

  testWidgets('the password minimum comes from the node: 8 by default, so a 6-7 character password never reaches it', (t) async {
    final api = WizardApi()..minPassword = null; // узел не назвал минимум — действует строгий по умолчанию
    await start(t, api);
    await toPasswordStep(t, seed24.split(' '));
    await t.enterText(find.byType(TextField).at(0), '1234567');
    await t.enterText(find.byType(TextField).at(1), '1234567');
    await click(t, 'Создать узел');
    expect(find.text('Пароль — не короче 8 символов'), findsOneWidget);
    expect(api.calls.contains('/api/auth/setup'), false);
    await stop(t);
  });

  testWidgets('finish creates the keys from the words and then the closed environment, then offers the place for backups', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toPasswordStep(t, seed24.split(' '));
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'long password');
    await click(t, 'Создать узел');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    final setup = api.bodies.firstWhere((b) => b.containsKey('login_password'));
    expect(setup['master_password'], seed24);
    expect(setup['master_password_repeat'], seed24);
    expect(setup['login_password'], 'long password');
    final vault = api.bodies.lastWhere((b) => b.containsKey('phrase'));
    expect(vault, {'password': 'long password', 'phrase': seed24});
    expect(api.calls.indexOf('/api/auth/setup') < api.calls.indexOf('/api/vault/setup'), true);
    // после сида и пароля — окно «где хранить копии»: сеанс администратора открыт тем же паролем; можно пропустить
    expect(find.text('Копии связей'), findsOneWidget);
    expect(api.bodies.any((b) => b['password'] == 'long password' && !b.containsKey('phrase') && !b.containsKey('login_password')), true);
    await click(t, 'Пропустить (напомню позже)');
    expect(find.text('Копии связей'), findsNothing);
    expect(api.calls.last == '/api/admin/session' || api.calls.contains('/api/admin/session'), true, reason: 'сеанс администратора закрыт после окна');
    await stop(t);
  });

  testWidgets('a control word travels with the phrase as one string (after U+001F) to the keys and to the closed environment', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: seed24.split(' '));
    await typeAsked(t, seed24.split(' '));
    await click(t, 'ОК');
    await t.enterText(find.byType(TextField).at(0), 'Воробей-2026');
    await t.enterText(find.byType(TextField).at(1), 'Воробей-2026');
    await click(t, 'Далее');
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'long password');
    await click(t, 'Создать узел');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    final joined = '$seed24\u001fВоробей-2026';
    final setup = api.bodies.firstWhere((b) => b.containsKey('login_password'));
    expect(setup['master_password'], joined);
    expect(setup['master_password_repeat'], joined);
    expect(api.bodies.lastWhere((b) => b.containsKey('phrase'))['phrase'], joined);
    await click(t, 'Пропустить (напомню позже)');
    await stop(t);
  });

  testWidgets('отдельный пароль администратора уходит на узел; одинаковый с паролем пользователя отвергается', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toPasswordStep(t, seed24.split(' '));
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'long password');
    await t.enterText(find.byType(TextField).at(2), 'long password');
    await t.enterText(find.byType(TextField).at(3), 'long password');
    await click(t, 'Создать узел');
    expect(find.text('Пароль администратора должен отличаться от пароля пользователя'), findsOneWidget);
    expect(api.calls.contains('/api/auth/setup'), false);
    await t.enterText(find.byType(TextField).at(2), 'admin password');
    await t.enterText(find.byType(TextField).at(3), 'admin passwordX');
    await click(t, 'Создать узел');
    expect(find.text('Пароли администратора не совпадают'), findsOneWidget);
    await t.enterText(find.byType(TextField).at(3), 'admin password');
    await click(t, 'Создать узел');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    final setup = api.bodies.firstWhere((b) => b.containsKey('login_password'));
    expect(setup['admin_password'], 'admin password');
    expect(setup['admin_password_repeat'], 'admin password');
    expect(setup['login_password'], 'long password');
    // окно копий открывается сеансом администратора, а не паролем входа
    expect(api.bodies.any((b) => b['password'] == 'admin password' && !b.containsKey('phrase') && !b.containsKey('login_password')), true);
    await click(t, 'Пропустить (напомню позже)');
    await stop(t);
  });

  testWidgets('the language button switches the instructions, the words stay English', (t) async {
    final api = WizardApi();
    await start(t, api);
    await t.tap(find.text('English'));
    await t.pumpAndSettle();
    expect(find.text('Step 1 of 5'), findsOneWidget);
    expect(find.text('I saved the words'), findsOneWidget);
    expect(find.text('Restore with a seed phrase'), findsOneWidget);
    expect(find.textContaining('art'), findsWidgets);
    await t.tap(find.text('Русский'));
    await t.pumpAndSettle();
    expect(find.text('Шаг 1 из 5'), findsOneWidget);
    await stop(t);
  });

  testWidgets('the control word step speaks English too', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: seed24.split(' '));
    await typeAsked(t, seed24.split(' '));
    await click(t, 'ОК');
    await t.tap(find.text('English'));
    await t.pumpAndSettle();
    expect(find.text('Step 4 of 5'), findsOneWidget);
    expect(find.text('Without a control word'), findsOneWidget);
    expect(find.text('Next'), findsOneWidget);
    expect(find.textContaining('Without it the seed phrase will not restore the node'), findsOneWidget);
    await stop(t);
  });

  testWidgets('restore by the seed phrase alone: the node number is shown first and must be confirmed; then the same keys are made, no file needed', (t) async {
    final api = WizardApi();
    await start(t, api);
    await click(t, 'Восстановить по сид-фразе');
    final fields = find.byType(TextField);
    await t.enterText(fields.at(0), 'one two three');
    await click(t, 'Проверить номер узла');
    expect(find.text('Введите 12, 18 или 24 слова сид-фразы'), findsOneWidget);
    await t.enterText(fields.at(0), 'bad ${seed12.split(' ').skip(1).join(' ')}');
    await click(t, 'Проверить номер узла');
    expect(find.textContaining('Фраза или контрольное слово не прошли проверку'), findsOneWidget);
    await t.enterText(fields.at(0), seed12);
    await t.enterText(fields.at(1), 'Воробей-2026');
    await t.enterText(fields.at(2), 'long password');
    await t.enterText(fields.at(3), 'long password');
    // без проверки номера восстановить нельзя
    await click(t, 'Восстановить');
    expect(find.text('Сначала проверьте номер узла и подтвердите, что он совпадает с вашей записью'), findsOneWidget);
    await click(t, 'Проверить номер узла');
    expect(find.textContaining('0123 4567 89ab cdef'), findsOneWidget);
    expect(api.bodies.any((b) => b['phrase'] == '$seed12\u001fВоробей-2026' && b.length == 1), true, reason: 'предпросмотр получил слова и слово одной строкой');
    await click(t, 'Восстановить');
    expect(find.text('Сначала проверьте номер узла и подтвердите, что он совпадает с вашей записью'), findsOneWidget, reason: 'номер показан, но не подтверждён');
    await t.tap(find.byKey(const ValueKey('id-matches')));
    await t.pump();
    await click(t, 'Восстановить');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    final setup = api.bodies.firstWhere((b) => b.containsKey('login_password'));
    expect(setup['master_password'], '$seed12\u001fВоробей-2026');
    expect(setup['login_password'], 'long password');
    expect(api.calls.contains('/api/auth/restore'), false, reason: 'файла нет — это обычное создание тех же ключей');
    expect(api.bodies.lastWhere((b) => b.containsKey('phrase') && b.containsKey('password'))['phrase'], '$seed12\u001fВоробей-2026');
    await click(t, 'Пропустить (напомню позже)');
    await stop(t);
  });

  testWidgets('the changed phrase or word drops the confirmed node number', (t) async {
    final api = WizardApi();
    await start(t, api);
    await click(t, 'Восстановить по сид-фразе');
    final fields = find.byType(TextField);
    await t.enterText(fields.at(0), seed12);
    await click(t, 'Проверить номер узла');
    await t.tap(find.byKey(const ValueKey('id-matches')));
    await t.pump();
    await t.enterText(fields.at(1), 'Воробей-2026');
    await t.pump();
    expect(find.byKey(const ValueKey('id-matches')), findsNothing, reason: 'после правки номер надо проверить заново');
    await stop(t);
  });

  testWidgets('restore with a links file: the box enables the attach button, the file belongs to the same node, then the vault opens with the new password', (t) async {
    final api = WizardApi();
    await start(t, api);
    await click(t, 'Восстановить по сид-фразе');
    expect(find.textContaining('Пароль прежней установки не переносится'), findsOneWidget);
    expect(find.text('Прикрепить файл'), findsNothing, reason: 'пока флажок не стоит, файла нет');
    final fields = find.byType(TextField);
    await t.enterText(fields.at(0), seed12);
    await click(t, 'Проверить номер узла');
    await t.tap(find.byKey(const ValueKey('id-matches')));
    await t.pump();
    await t.tap(find.byKey(const ValueKey('has-file')));
    await t.pumpAndSettle();
    expect(find.text('Прикрепить файл'), findsOneWidget);
    // копия другого узла отвергается до записи
    api.inspectNodeId = 'ffffffffffffffff';
    await t.enterText(find.widgetWithText(TextField, 'Путь к файлу копии'), '/home/iam/копия.yandibak');
    await t.testTextInput.receiveAction(TextInputAction.done);
    await t.pumpAndSettle();
    expect(find.text('Копия принадлежит другому узлу: проверьте слова фразы и контрольное слово.'), findsOneWidget);
    api.inspectNodeId = '0123456789abcdef';
    await t.enterText(find.widgetWithText(TextField, 'Путь к файлу копии'), '/home/iam/копия.yandibak');
    await t.testTextInput.receiveAction(TextInputAction.done);
    await t.pumpAndSettle();
    expect(find.textContaining('Копия №3'), findsOneWidget);
    await t.enterText(find.widgetWithText(TextField, 'Новый пароль'), 'long password');
    await t.enterText(find.widgetWithText(TextField, 'Повтор нового пароля'), 'other password');
    await click(t, 'Восстановить');
    expect(find.text('Пароли не совпадают'), findsOneWidget);
    expect(api.calls.contains('/api/auth/restore'), false);
    await t.enterText(find.widgetWithText(TextField, 'Повтор нового пароля'), 'long password');
    await click(t, 'Восстановить');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    expect(api.bodies.firstWhere((b) => b.containsKey('path') && b.containsKey('phrase')),
        {'path': '/home/iam/копия.yandibak', 'phrase': seed12, 'password': 'long password'});
    expect(api.calls.indexOf('/api/auth/restore') < api.calls.indexOf('/api/vault/unlock'), true);
    await click(t, 'Пропустить (напомню позже)');
    await stop(t);
  });

  Future<void> restoreWithFile(WidgetTester t, WizardApi api) async {
    await start(t, api);
    await click(t, 'Восстановить по сид-фразе');
    final fields = find.byType(TextField);
    await t.enterText(fields.at(0), seed12);
    await click(t, 'Проверить номер узла');
    await t.tap(find.byKey(const ValueKey('id-matches')));
    await t.pump();
    await t.tap(find.byKey(const ValueKey('has-file')));
    await t.pumpAndSettle();
    await t.enterText(find.widgetWithText(TextField, 'Путь к файлу копии'), '/home/iam/копия.yandibak');
    await t.testTextInput.receiveAction(TextInputAction.done);
    await t.pumpAndSettle();
    await t.enterText(find.widgetWithText(TextField, 'Новый пароль'), 'long password');
    await t.enterText(find.widgetWithText(TextField, 'Повтор нового пароля'), 'long password');
    await click(t, 'Восстановить');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
  }

  testWidgets('a failed restore after files were written still shows the paths of what was not cleaned (P19-3)', (t) async {
    final api = WizardApi()..restoreFails = const ApiFailure('http', 400, ['/home/iam/.yandi/.paired_clients.json.restore-1-0: не разбирается'], true);
    await restoreWithFile(t, api);
    expect(find.textContaining('.paired_clients.json.restore-1-0'), findsOneWidget, reason: 'путь остатка показан владельцу');
    expect(find.textContaining('Часть файлов копии записана'), findsOneWidget);
    await stop(t);
  });

  testWidgets('a failed restore with files written but no warnings still says that files are on disk (P20-4)', (t) async {
    final api = WizardApi()..restoreFails = const ApiFailure('http', 400, [], true);
    await restoreWithFile(t, api);
    expect(find.textContaining('Часть файлов копии записана'), findsOneWidget);
    await stop(t);
  });

  testWidgets('the seed sheet is ONLY the numbered words, goes to the printer from memory, and a failure is told', (t) async {
    final api = WizardApi();
    final sent = <String>[];
    var ok = true;
    await t.pumpWidget(MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: PreferencesScope(
            preferences: AppPreferences(),
            child: SetupWizard(
              api: api,
              preferences: AppPreferences(),
              busy: false,
              finish: (_, __, ___) async {},
              restore: (_, __, ___) async {},
              printer: (text) async {
                sent.add(text);
                return ok;
              },
            ),
          ),
        ),
      ),
    ));
    await t.pumpAndSettle();
    await click(t, 'Распечатать');
    final lines = sent.single.trimRight().split('\n');
    expect(lines.length, 24, reason: 'на листе только слова: ни заголовка, ни памятки, ни слова-пароля, ни номера узла');
    expect(lines.first.trim(), '1. abandon');
    expect(lines.last.trim(), '24. art');
    expect(sent.single.contains('YANDI') || sent.single.toLowerCase().contains('seed') || sent.single.toLowerCase().contains('phrase'), false);
    expect(find.textContaining('Отправлено на печать'), findsOneWidget);
    ok = false;
    await click(t, 'Распечатать');
    expect(find.textContaining('Печать недоступна'), findsOneWidget);
    await stop(t);
  });

  testWidgets('leaving the window hides the seed words, and a long absence recreates the wizard without them', (t) async {
    final api = CountingDesignApi('setup');
    t.view.physicalSize = const Size(1280, 800);
    t.view.devicePixelRatio = 1;
    addTearDown(t.view.resetPhysicalSize);
    addTearDown(t.view.resetDevicePixelRatio);
    await t.pumpWidget(DesktopApp(api: api));
    await t.pumpAndSettle();
    expect(find.textContaining('abandon'), findsWidgets, reason: 'слова показаны, пока окно активно');
    expect(api.seedCalls, 1);
    // ушли в другое окно: слова не рисуются, состояние мастера цело
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await t.pump();
    expect(find.textContaining('abandon'), findsNothing, reason: 'в неактивном окне сид не на экране');
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pumpAndSettle();
    expect(find.textContaining('abandon'), findsWidgets);
    expect(api.seedCalls, 1, reason: 'недолгий уход мастер не сбрасывает');
    // долгое отсутствие: мастер создаётся заново (прежние слова и ввод не сохраняются)
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await t.pump(const Duration(minutes: 3, seconds: 1));
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pumpAndSettle();
    expect(api.seedCalls, 2, reason: 'новый мастер запросил новые слова');
    await stop(t);
  });
}

class CountingDesignApi extends DesignApi {
  int seedCalls = 0;
  CountingDesignApi(super.scene);
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) {
    if (path == '/api/auth/seed/new') seedCalls++;
    return super.request(path, body: body, method: method);
  }
}

