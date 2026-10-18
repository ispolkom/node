import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'shell_m2_test.dart' as m2;
import 'shell_m2_test.dart' show FeatureApi;
import 'widget_test.dart' show FakeApi, start, click, stop, admin;

void main() {
  testWidgets('личное меню без управления узлом, с входом в админ-панель', (t) async {
    final api = FakeApi();
    await start(t, api);
    await click(t, 'Войти');
    for (final hidden in ['Жалобы', 'Шлюзы', 'Статус узла', 'Настройки узла']) {
      expect(find.text(hidden), findsNothing, reason: hidden);
    }
    expect(find.textContaining('Админ-панель'), findsWidgets);
    await stop(t);
  });

  testWidgets('админ-панель: пароль, меню с «Личный аккаунт», выход закрывает сеанс', (t) async {
    final api = FakeApi();
    await start(t, api);
    await click(t, 'Войти');
    await admin(t, 'Статус узла');
    expect(api.adminActive, true);
    expect(find.text('Личный аккаунт'), findsWidgets);
    expect(find.text('Жалобы'), findsWidgets);
    expect(find.text('Настройки узла'), findsWidgets);
    await click(t, 'Личный аккаунт');
    expect(api.adminActive, false);
    expect(find.text('Жалобы'), findsNothing);
    await stop(t);
  });

  testWidgets('извещения на главной: тип и число, без текста', (t) async {
    final api = FakeApi()
      ..notices = [
        {'kind': 'complaint', 'label': 'Новые жалобы', 'count': 2, 'new': true},
      ];
    await start(t, api);
    await click(t, 'Войти');
    expect(find.textContaining('Новые жалобы'), findsWidgets);
    expect(find.textContaining('Админ-панель (1)'), findsWidgets);
    await stop(t);
  });

  testWidgets('смена пароля администратора: несовпадение не уходит на узел, верный ввод уходит', (t) async {
    final api = FeatureApi();
    await m2.openAdmin(t, api, 'Настройки узла');
    Finder f(String l) => find.widgetWithText(TextField, l);
    await t.ensureVisible(f('Текущий пароль администратора'));
    await t.enterText(f('Текущий пароль администратора'), 'admin-pass');
    await t.enterText(f('Новый пароль администратора'), 'new admin pass');
    await t.enterText(f('Повтор нового пароля'), 'другой');
    await click(t, 'Сохранить пароль администратора');
    expect(find.text('Пароли не совпадают'), findsWidgets);
    expect(api.calls.contains('/api/admin/password'), false);
    await t.enterText(f('Повтор нового пароля'), 'new admin pass');
    await click(t, 'Сохранить пароль администратора');
    expect(api.calls.contains('/api/admin/password'), true);
    expect(find.text('Пароль администратора сохранён.'), findsOneWidget);
    await stop(t);
  });
}
