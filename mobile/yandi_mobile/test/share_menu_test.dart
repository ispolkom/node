import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/widgets/message_menu.dart';

void main() {
  testWidgets('share picker lists contacts and groups and returns the choice', (t) async {
    ShareTarget? picked;
    await t.pumpWidget(MaterialApp(
      home: Builder(
        builder: (ctx) => TextButton(
          onPressed: () async => picked = await pickShareTarget(ctx, [
            ShareTarget.contact('c' * 64, 'Алиса'),
            ShareTarget.group('g' * 32, 'Семья'),
          ]),
          child: const Text('open'),
        ),
      ),
    ));
    await t.tap(find.text('open'));
    await t.pumpAndSettle();
    expect(find.text('Алиса'), findsOneWidget);
    expect(find.text('Семья'), findsOneWidget);
    await t.tap(find.text('Семья'));
    await t.pumpAndSettle();
    expect(picked!.group, true);
    expect(picked!.id, 'g' * 32);
  });
  testWidgets('empty list says there is nobody to share with', (t) async {
    await t.pumpWidget(MaterialApp(
      home: Builder(
        builder: (ctx) => TextButton(
          onPressed: () => pickShareTarget(ctx, []),
          child: const Text('open'),
        ),
      ),
    ));
    await t.tap(find.text('open'));
    await t.pumpAndSettle();
    expect(find.text('Некому пересылать'), findsOneWidget);
  });
}
