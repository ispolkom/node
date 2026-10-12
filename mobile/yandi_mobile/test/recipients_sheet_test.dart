import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/theme.dart';
import 'package:yandi_mobile/widgets/recipients_sheet.dart';

Contact c(String id, String name, {bool online = false, bool saved = true}) =>
    Contact(peerId: id, displayName: name, online: online, isManual: false, saved: saved);

void main() {
  test('кандидаты: без исходного чата, сохранённые выше, в сети выше', () {
    final list = forwardCandidates([
      c('a', 'Яна'),
      c('b', 'Борис', online: true),
      c('self', 'Чат'),
      c('d', 'Дима', saved: false, online: true),
    ], exclude: 'self');
    expect(list.map((x) => x.peerId), ['b', 'a', 'd']);
  });

  testWidgets('до 5 получателей за раз, шестого не добавить', (t) async {
    final contacts = [for (var i = 0; i < 8; i++) c('p$i', 'Друг $i', online: true)];
    List<Contact>? result;
    await t.pumpWidget(MaterialApp(
      theme: AppTheme.dark,
      home: Scaffold(
        body: Builder(
          builder: (ctx) => TextButton(
            onPressed: () async => result = await pickRecipients(ctx, title: 'Переслать', contacts: contacts),
            child: const Text('open'),
          ),
        ),
      ),
    ));
    await t.tap(find.text('open'));
    await t.pumpAndSettle();
    expect(find.text('Выберите получателей'), findsOneWidget);
    for (var i = 0; i < 6; i++) {
      final f = find.byKey(ValueKey('rcpt-p$i'), skipOffstage: false);
      t.widget<CheckboxListTile>(f).onChanged!(true);
      await t.pump();
    }
    expect(find.text('Отправить (5)'), findsOneWidget);
    expect(find.text('Не больше 5 получателей за раз'), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('rcpt-send')));
    await t.pumpAndSettle();
    expect(result!.length, 5);
  });
}
