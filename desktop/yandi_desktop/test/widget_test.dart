import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/app.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'shell_m2_test.dart' as m2;

class FakeApi implements Api {
  bool setup = false, recovery = false, down = false;
  bool configured = true, unlocked = true;
  int secs = 0, cancels = 0;
  String? failPath;
  final calls = <String>[];
  final bodies = <Map<String, dynamic>>[];
  Completer<Map<String, dynamic>>? pending;
  Map<String, dynamic> get vault => {
    'configured': configured,
    'unlocked': unlocked,
    'auto_lock_secs': secs,
    'migration_pending': false,
    'error': null,
  };
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    calls.add(path);
    if (body != null) bodies.add(Map.of(body));
    if (path == failPath) throw const ApiFailure('http', 401);
    if (down) throw const ApiFailure('disconnected');
    if (path == '/api/auth/state')
      return {'setup_required': setup, 'recovery_required': recovery};
    if (path == '/api/status') return {'short_id': 'abc', 'status': 'online'};
    if (path == '/api/nodes') return {'nodes': []};
    if (path == '/api/speed') return {'rx_speed': 12};
    if (path == '/api/vault/status') return vault;
    if (pending != null) return pending!.future;
    if (path == '/api/vault/setup') {
      configured = unlocked = true;
    }
    if (path == '/api/vault/unlock' || path == '/api/vault/recover')
      unlocked = true;
    if (path == '/api/vault/lock') unlocked = false;
    if (path == '/api/vault/auto_lock') secs = body!['secs'] as int;
    return {'ok': true, 'status': vault};
  }

  @override
  void cancel() {
    cancels++;
  }
}

Future<void> start(WidgetTester tester, FakeApi api) async {
  await tester.pumpWidget(DesktopApp(api: api));
  await tester.pumpAndSettle();
}

Finder action(String label) {
  final byText = find.ancestor(
    of: find.text(label),
    matching: find.byWidgetPredicate((w) => w is ButtonStyleButton),
  );
  // значок без слова (отправка) находится по подсказке
  if (byText.evaluate().isNotEmpty) return byText.first;
  final tip = find.byTooltip(label);
  return tip.evaluate().isNotEmpty ? tip.first : find.text(label).first;
}
Future<void> click(WidgetTester tester, String label) async {
  await tester.pump(); // введённый текст меняет микрофон на самолётик
  final button = action(label);
  await tester.ensureVisible(button);
  await tester.tap(button);
  await tester.pumpAndSettle();
}

/// Message commands are words in a popup menu: tap the message, then the word.
Future<void> command(WidgetTester tester, String message, String label) async {
  await tester.ensureVisible(find.text(message).first);
  await tester.tap(find.text(message).first);
  await tester.pumpAndSettle();
  await tester.tap(find.text(label).last);
  await tester.pumpAndSettle();
}

/// Words-as-commands (attachment actions): tap the word itself.
Future<void> word(WidgetTester tester, String label) async {
  await tester.ensureVisible(find.text(label).first);
  await tester.tap(find.text(label).first);
  await tester.pumpAndSettle();
}

/// Иконка-кнопка с подсказкой (в окне чата вместо слов-ссылок).
Future<void> tip(WidgetTester tester, String label) async {
  await tester.ensureVisible(find.byTooltip(label).first);
  await tester.tap(find.byTooltip(label).first);
  await tester.pumpAndSettle();
}

Future<void> vaultPage(WidgetTester tester) async {
  await click(tester, 'Войти');
  await click(tester, 'Настройки');
}

Future<void> stop(WidgetTester tester) => tester.pumpWidget(const SizedBox());
void main() {
  testWidgets('offline error and manual retry', (t) async {
    final api = FakeApi()..down = true;
    await start(t, api);
    expect(find.text('Нет связи с демоном'), findsOneWidget);
    api.down = false;
    await click(t, 'Повторить подключение');
    expect(find.text('Доступ к ноде'), findsOneWidget);
    await stop(t);
  });
  testWidgets(
    'setup hidden fields, Unicode validation and single flight Enter',
    (t) async {
      final api = FakeApi()..setup = true;
      await start(t, api);
      final fields = find.byType(TextField);
      expect(fields, findsNWidgets(3));
      for (final field in t.widgetList<TextField>(fields)) {
        expect(
          field.obscureText &&
              field.autocorrect == false &&
              !field.enableSuggestions,
          true,
        );
        expect(field.autofillHints, isEmpty);
      }
      final inputs = [
        'пароль12',
        'пароль12',
        'первое второе третье четвёртое',
      ];
      for (var i = 0; i < 3; i++) {
        await t.enterText(fields.at(i), inputs[i]);
      }
      api.pending = Completer<Map<String, dynamic>>();
      await t.ensureVisible(fields.last);
      await t.testTextInput.receiveAction(TextInputAction.done);
      await t.pump();
      await t.testTextInput.receiveAction(TextInputAction.done);
      await t.pump();
      expect(api.calls.where((c) => c == '/api/auth/setup').length, 1);
      expect(
        t
            .widgetList<TextField>(fields)
            .every((f) => f.controller!.text.isEmpty),
        true,
      );
      api.setup = false;
      api.pending!.complete({'ok': true});
      await t.pump();
      await t.pump(const Duration(seconds: 1));
      await t.pumpAndSettle();
      expect(find.text('Доступ к ноде'), findsOneWidget);
      await stop(t);
    },
  );
  testWidgets('recovery state explains CLI binding recovery', (t) async {
    await start(t, FakeApi()..recovery = true);
    expect(find.text('Требуется восстановление'), findsOneWidget);
    expect(find.textContaining('yandi-keys recover'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Войти'), findsNothing);
    await stop(t);
  });
  testWidgets('dashboard polls stop hidden and refresh resumed', (t) async {
    final api = FakeApi();
    await start(t, api);
    await click(t, 'Войти');
    expect(find.text('Статус узла'), findsOneWidget);
    expect(find.text('abc'), findsOneWidget);
    await t.pump(const Duration(seconds: 10));
    await t.pumpAndSettle();
    expect(
      api.calls.where((p) => p == '/api/vault/status').length,
      greaterThan(1),
    );
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    await t.pumpAndSettle();
    final count = api.calls.length;
    await t.pump(const Duration(seconds: 20));
    expect(api.calls.length, count);
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pumpAndSettle();
    expect(api.calls.length, greaterThan(count));
    await stop(t);
  });
  testWidgets('vault setup requires four secret words', (t) async {
    final api = m2.FeatureApi()..configured = false..unlocked = false;
    await start(t, api);
    await vaultPage(t);
    await t.enterText(find.byType(TextField).at(0), 'owner password');
    await t.enterText(find.byType(TextField).at(1), 'owner password');
    await click(t, 'Создать закрытую среду');
    expect(find.byType(AlertDialog), findsNothing);
    expect(api.calls.contains('/api/vault/setup'), false);
    await t.enterText(find.byType(TextField).at(0), 'owner password');
    await t.enterText(find.byType(TextField).at(1), 'owner password');
    await t.enterText(find.byType(TextField).at(2), 'alpha1 bravo2 charlie3 delta4');
    await click(t, 'Создать закрытую среду');
    expect(api.bodies.last, {
      'password': 'owner password',
      'phrase': 'alpha1 bravo2 charlie3 delta4',
    });
    expect(
      find.text('Открыта. Узел может читать историю и группы ПК.'),
      findsOneWidget,
    );
    await stop(t);
  });
  testWidgets('vault unlock, change, timeout, lock, phrase recovery', (
    t,
  ) async {
    final api = m2.FeatureApi()
      ..configured = true
      ..unlocked = false;
    await start(t, api);
    await vaultPage(t);
    await t.enterText(find.byType(TextField).first, 'owner password');
    await click(t, 'Разблокировать');
    expect(api.bodies.last, {'password': 'owner password'});
    final fields = find.byType(TextField);
    for (var i = 0; i < 3; i++) {
      await t.enterText(
        fields.at(i),
        i == 0 ? 'owner password' : 'new password',
      );
    }
    await click(t, 'Сменить пароль');
    expect(api.bodies.last, {
      'old_password': 'owner password',
      'new_password': 'new password',
    });
    await click(t, 'Никогда не блокировать'); // по умолчанию в тесте выключена: включаем таймер
    await t.enterText(
      find.descendant(
        of: find.byType(TimeoutForm),
        matching: find.byType(TextField),
      ),
      '60',
    );
    await click(t, 'Сохранить автоблокировку');
    expect(api.secs, 3600); // 60 минут
    await click(t, 'Запереть');
    expect(find.textContaining('Закрытая среда заперта'), findsOneWidget);
    for (var i = 1; i <= 3; i++) {
      await t.enterText(
        find.byType(TextField).at(i),
        i == 1 ? 'alpha1 bravo2 charlie3 delta4' : 'recovered password',
      );
    }
    await click(t, 'Восстановить словами');
    expect(api.bodies.last, {
      'phrase': 'alpha1 bravo2 charlie3 delta4',
      'new_password': 'recovered password',
    });
    await stop(t);
  });
  testWidgets(
    'remote lock clears focused secrets; late unlock ignored after hide',
    (t) async {
      final api = m2.FeatureApi()
        ..configured = true
        ..unlocked = true;
      await start(t, api);
      await vaultPage(t);
      await t.enterText(find.byType(TextField).first, 'old secret');
      api.unlocked = false;
      await t.pump(const Duration(seconds: 1));
      await t.pumpAndSettle();
      expect(
        t
            .widgetList<TextField>(find.byType(TextField))
            .every((f) => f.controller!.text != 'old secret'),
        true,
      );
      api.pending = Completer<Map<String, dynamic>>();
      await t.enterText(find.byType(TextField).first, 'unlock secret');
      await t.ensureVisible(action('Разблокировать'));
      await t.tap(action('Разблокировать'));
      await t.pump();
      t.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      await t.pump();
      api.pending!.complete({'ok': true});
      await t.pump();
      expect(
        find.text('Открыта. Узел может читать историю и группы ПК.'),
        findsNothing,
      );
      t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await t.pumpAndSettle();
      await stop(t);
    },
  );
  testWidgets('failed lock stays closed locally until explicit retry', (
    t,
  ) async {
    final api = m2.FeatureApi()
      ..configured = true
      ..unlocked = true;
    await start(t, api);
    await vaultPage(t);
    api.failPath = '/api/vault/lock';
    await click(t, 'Запереть');
    expect(find.text('Состояние блокировки не подтверждено'), findsOneWidget);
    await t.pump(const Duration(seconds: 5));
    await t.pumpAndSettle();
    expect(
      find.text('Открыта. Узел может читать историю и группы ПК.'),
      findsNothing,
    );
    api.failPath = null;
    await click(t, 'Повторить блокировку');
    expect(find.textContaining('Закрытая среда заперта'), findsOneWidget);
    await stop(t);
  });
  testWidgets('logout locks the vault even when the cached state says it is already locked', (t) async {
    final api = FakeApi()..configured = true..unlocked = false;
    await start(t, api);
    await click(t, 'Войти');
    expect(api.calls.contains('/api/vault/lock'), false);
    await t.tap(find.byTooltip('Выйти (запирает закрытую среду)'));
    await t.pumpAndSettle();
    expect(api.calls.contains('/api/vault/lock'), true, reason: 'выход всегда запирает среду');
    await stop(t);
  });
  testWidgets('password mismatch does not send setup command', (t) async {
    final api = FakeApi()..setup = true;
    await start(t, api);
    await t.enterText(find.byType(TextField).at(0), 'long password');
    await t.enterText(find.byType(TextField).at(1), 'other password');
    await click(t, 'Создать и запустить ноду');
    expect(find.text('Пароли не совпадают'), findsOneWidget);
    expect(api.calls.contains('/api/auth/setup'), false);
    await stop(t);
  });
  testWidgets('auth recovery sends existing API contract', (t) async {
    final api = FakeApi();
    await start(t, api);
    final inputs = [
      'test master phrase',
      'new login password',
      'new login password',
    ];
    for (var i = 0; i < 3; i++) {
      await t.enterText(find.byType(TextField).at(i), inputs[i]);
    }
    await click(t, 'Восстановить доступ');
    expect(api.bodies.last, {
      'recovery_code': 'test master phrase',
      'new_login_password': 'new login password',
      'remember_me': false,
    });
    await stop(t);
  });
  testWidgets('small window and invalid auto-lock stay usable', (t) async {
    await t.binding.setSurfaceSize(const Size(400, 500));
    addTearDown(() => t.binding.setSurfaceSize(null));
    final api = m2.FeatureApi()
      ..configured = true
      ..unlocked = true;
    await start(t, api);
    await vaultPage(t);
    await click(t, 'Никогда не блокировать');
    await t.enterText(
      find.descendant(
        of: find.byType(TimeoutForm),
        matching: find.byType(TextField),
      ),
      '101',
    );
    await click(t, 'Сохранить автоблокировку');
    expect(find.text('Введите число от 1 до 100'), findsOneWidget);
    expect(api.calls.contains('/api/vault/auto_lock'), false);
    expect(t.takeException(), isNull);
    await stop(t);
  });
}
