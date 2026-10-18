import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
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
}
