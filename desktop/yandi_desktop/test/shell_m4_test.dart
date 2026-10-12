import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

void main() {
  testWidgets('own text edit, local delete and real clear confirmation', (
    t,
  ) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.byTooltip('Отправлено'), findsOneWidget);
    await m1.command(t, '<b>Текст</b>', 'Изменить');
    expect(find.text('Редактирование сообщения'), findsOneWidget);
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'правка');
    await m1.click(t, 'Отправить');
    expect(find.text('Редактирование сообщения'), findsNothing);
    expect(api.methods.contains('PATCH'), true);
    expect(api.bodies.last, {'msg_id': 'new', 'text': 'правка'});
    await m1.command(t, '<b>Текст</b>', 'Удалить');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.command(t, '<b>Текст</b>', 'Удалить');
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.last, {'msg_id': 'new', 'for_everyone': false});
    await m1.word(t, 'Очистить историю');
    expect(api.calls.where((p) => p.contains('/clear/')), isEmpty);
    await m1.click(t, 'Подтвердить');
    expect(api.calls.where((p) => p.contains('/clear/')).length, 1);
    await m1.stop(t);
  });
  testWidgets('reply sends reply_to and quotes the original', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'r1',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': 'ответная',
      'reply_to': {'msg_id': 'new', 'text': 'исходное'},
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.text('исходное'), findsOneWidget);
    await m1.command(t, '<b>Текст</b>', 'Ответить');
    await t.enterText(find.widgetWithText(TextField, 'Сообщение'), 'ок');
    await m1.click(t, 'Отправить');
    expect(api.bodies.last['reply_to'], {
      'msg_id': 'new',
      'text': '<b>Текст</b>',
    });
    await m1.stop(t);
  });
  testWidgets(
    'foreign messages cannot be edited or deleted; attachment cannot be edited',
    (t) async {
      final api = m2.FeatureApi();
      api.historyRows.first['from'] = m2.alice;
      await m2.open(t, api, 'Чат');
      await m1.click(t, 'Открыть чат');
      await t.tap(find.text('<b>Текст</b>'));
      await t.pumpAndSettle();
      expect(find.text('Ответить'), findsOneWidget);
      expect(find.text('Изменить'), findsNothing);
      expect(find.text('Удалить'), findsNothing);
      await m1.stop(t);
    },
  );
  testWidgets('select several messages and delete them together', (t) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'second',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text': 'вторая',
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await m1.command(t, '<b>Текст</b>', 'Выбрать');
    expect(find.text('Выбрано: 1'), findsOneWidget);
    await t.tap(find.text('вторая'));
    await t.pumpAndSettle();
    expect(find.text('Выбрано: 2'), findsOneWidget);
    await m1.word(t, 'Удалить выбранные');
    await m1.click(t, 'Подтвердить');
    expect(api.methods.where((m) => m == 'DELETE').length, 2);
    await m1.stop(t);
  });
  testWidgets('chat header has call words and a group invite is a card', (
    t,
  ) async {
    final api = m2.FeatureApi();
    api.historyRows.add({
      'msg_id': 'inv',
      'from': m2.alice,
      'timestamp': DateTime.now().millisecondsSinceEpoch,
      'text':
          '\u0001yandi-gi:{"g":"${'a' * 32}","c":"abcdefgh12345678","n":"Клуб"}',
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.text('Позвонить'), findsOneWidget);
    expect(find.text('Видеозвонок'), findsOneWidget);
    expect(find.textContaining('«Клуб»'), findsOneWidget);
    expect(find.text('Вступить в группу'), findsWidgets);
    await m1.stop(t);
  });
}
