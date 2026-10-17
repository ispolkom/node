import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/setup_wizard.dart';

import 'widget_test.dart' show FakeApi, start, click, stop;

const seed12 = 'legal winner thank year wave sausage worth useful legal winner thank yellow';
final seed24 = '${List.filled(23, 'abandon').join(' ')} art';

/// Узел для мастера: отдаёт сид по запросу, ключи создаёт один раз.
class WizardApi extends FakeApi {
  final asked = <int>[];
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
      final n = (body?['words'] ?? 12) as int;
      asked.add(n);
      calls.add(path);
      return {'words': (n == 24 ? seed24 : seed12).split(' '), 'count': n};
    }
    if (path == '/api/auth/restore/inspect') {
      calls.add(path);
      bodies.add(Map.of(body!));
      return {'counter': 3, 'node_id': 'abc', 'text': 'Копия №3 от 2026-10-17 09:05 UTC; узел abc'};
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

void main() {
  testWidgets('wizard shows 12 words first, nothing is created until the password', (t) async {
    final api = WizardApi();
    await start(t, api);
    expect(find.text('Шаг 1 из 4'), findsOneWidget);
    expect(find.text('Выберите степень сложности сид-фразы'), findsOneWidget);
    expect(api.asked, [12]);
    expect(find.textContaining('yellow'), findsOneWidget);
    expect(api.calls.contains('/api/auth/setup'), false);
    await stop(t);
  });

  testWidgets('24 words on request, new phrase each time', (t) async {
    final api = WizardApi();
    await start(t, api);
    await t.tap(find.byType(DropdownButton<int>));
    await t.pumpAndSettle();
    await t.tap(find.text('24 слова (повышенная)').last);
    await t.pumpAndSettle();
    expect(api.asked, [12, 24]);
    expect(find.textContaining('art'), findsWidgets);
    await stop(t);
  });

  testWidgets('wrong click order is refused and asks to look again', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t, phrase: seed12.split(' ').reversed.toList());
    expect(find.text('Порядок не совпал. Вернитесь и сверьте слова с записью.'), findsOneWidget);
    expect(find.text('Шаг 2 из 4'), findsOneWidget);
    await click(t, 'Показать слова снова');
    expect(find.text('Шаг 1 из 4'), findsOneWidget);
    expect(find.textContaining('yellow'), findsOneWidget, reason: 'тот же сид, не новый');
    expect(api.asked, [12]);
    await stop(t);
  });

  testWidgets('three words by number are checked, then the password', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t);
    expect(find.text('Шаг 3 из 4'), findsOneWidget);
    expect(find.textContaining('yellow'), findsNothing, reason: 'слова на экране скрыты');
    await typeAsked(t, seed12.split(' '), wrong: true);
    await click(t, 'ОК');
    expect(find.text('Эти слова не совпали с записью. Проверьте запись.'), findsOneWidget);
    await typeAsked(t, seed12.split(' '));
    await click(t, 'ОК');
    expect(find.text('Шаг 4 из 4'), findsOneWidget);
    // пароли: разные и короткий не уходят на узел
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'other password');
    await click(t, 'Создать узел');
    expect(find.text('Пароли не совпадают'), findsOneWidget);
    await t.enterText(find.byType(TextField).at(0), '123');
    await t.enterText(find.byType(TextField).at(1), '123');
    await click(t, 'Создать узел');
    expect(find.text('Пароль — не короче 6 символов'), findsOneWidget);
    expect(api.calls.contains('/api/auth/setup'), false);
    for (final f in t.widgetList<TextField>(find.byType(TextField))) {
      expect(f.obscureText && f.autocorrect == false && !f.enableSuggestions, true);
    }
    await stop(t);
  });

  testWidgets('finish creates the keys from the words and then the closed environment', (t) async {
    final api = WizardApi();
    await start(t, api);
    await toTypeStep(t);
    await typeAsked(t, seed12.split(' '));
    await click(t, 'ОК');
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'long password');
    await click(t, 'Создать узел');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    final setup = api.bodies.firstWhere((b) => b.containsKey('login_password'));
    expect(setup['master_password'], seed12);
    expect(setup['master_password_repeat'], seed12);
    expect(setup['login_password'], 'long password');
    final vault = api.bodies.lastWhere((b) => b.containsKey('phrase'));
    expect(vault, {'password': 'long password', 'phrase': seed12});
    expect(api.calls.indexOf('/api/auth/setup') < api.calls.indexOf('/api/vault/setup'), true);
    await stop(t);
  });

  testWidgets('the language button switches the instructions, the words stay English', (t) async {
    final api = WizardApi();
    await start(t, api);
    await t.tap(find.text('English'));
    await t.pumpAndSettle();
    expect(find.text('Step 1 of 4'), findsOneWidget);
    expect(find.text('I saved the words'), findsOneWidget);
    expect(find.textContaining('yellow'), findsOneWidget);
    await t.tap(find.text('Русский'));
    await t.pumpAndSettle();
    expect(find.text('Шаг 1 из 4'), findsOneWidget);
    await stop(t);
  });

  testWidgets('restore from a backup: file, seed phrase, a NEW password; then the vault opens with it', (t) async {
    final api = WizardApi();
    await start(t, api);
    await click(t, 'Восстановить из копии');
    expect(find.textContaining('Пароль прежней установки не переносится'), findsOneWidget);
    final fields = find.byType(TextField);
    await t.enterText(fields.at(0), '/home/iam/копия.yandibak');
    await t.testTextInput.receiveAction(TextInputAction.done);
    await t.pumpAndSettle();
    expect(find.textContaining('Копия №3'), findsOneWidget);
    // неверные вводы не уходят на узел
    await t.enterText(fields.at(1), 'one two three');
    await t.enterText(fields.at(2), 'long password');
    await t.enterText(fields.at(3), 'long password');
    await click(t, 'Восстановить');
    expect(find.text('Введите 12 или 24 слова сид-фразы'), findsOneWidget);
    await t.enterText(fields.at(1), seed12);
    await t.enterText(fields.at(3), 'other password');
    await click(t, 'Восстановить');
    expect(find.text('Пароли не совпадают'), findsOneWidget);
    expect(api.calls.contains('/api/auth/restore'), false);
    await t.enterText(fields.at(1), seed12);
    await t.enterText(fields.at(2), 'long password');
    await t.enterText(fields.at(3), 'long password');
    await click(t, 'Восстановить');
    await t.pump(const Duration(seconds: 2));
    await t.pumpAndSettle();
    expect(api.bodies.firstWhere((b) => b.containsKey('path') && b.containsKey('phrase')),
        {'path': '/home/iam/копия.yandibak', 'phrase': seed12, 'password': 'long password'});
    expect(api.bodies.last, {'password': 'long password'}, reason: 'закрытая среда открывается новым паролем');
    expect(api.calls.indexOf('/api/auth/restore') < api.calls.indexOf('/api/vault/unlock'), true);
    await stop(t);
  });

  testWidgets('the seed sheet goes to the printer from memory, numbered, and a failure is told', (t) async {
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
              finish: (_, __) async {},
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
    expect(sent.single, contains(' 1. legal'));
    expect(sent.single, contains('12. yellow'));
    expect(find.textContaining('Отправлено на печать'), findsOneWidget);
    ok = false;
    await click(t, 'Распечатать');
    expect(find.textContaining('Печать недоступна'), findsOneWidget);
    await stop(t);
  });
}
