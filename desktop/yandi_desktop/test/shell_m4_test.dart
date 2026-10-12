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
    expect(find.textContaining('Отправлено'), findsOneWidget);
    await m1.click(t, 'Изменить сообщение');
    await t.enterText(
      find.widgetWithText(TextField, 'Текст сообщения'),
      'правка',
    );
    await m1.click(t, 'Сохранить');
    expect(api.methods.contains('PATCH'), true);
    expect(api.bodies.last, {'msg_id': 'new', 'text': 'правка'});
    await m1.click(t, 'Удалить сообщение');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.click(t, 'Удалить сообщение');
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.last, {'msg_id': 'new', 'for_everyone': false});
    await m1.click(t, 'Очистить историю');
    expect(api.calls.where((p) => p.contains('/clear/')), isEmpty);
    await m1.click(t, 'Подтвердить');
    expect(api.calls.where((p) => p.contains('/clear/')).length, 1);
    await m1.stop(t);
  });
  testWidgets(
    'foreign messages cannot be edited or deleted; attachment cannot be edited',
    (t) async {
      final api = m2.FeatureApi();
      api.historyRows.first['from'] = m2.alice;
      await m2.open(t, api, 'Чат');
      await m1.click(t, 'Открыть чат');
      expect(find.text('Изменить сообщение'), findsNothing);
      expect(find.text('Удалить сообщение'), findsNothing);
      await m1.stop(t);
    },
  );
}
