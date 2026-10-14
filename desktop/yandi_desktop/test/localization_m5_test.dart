import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/translations.dart';
import 'package:yandi_desktop/groups.dart';
import 'package:yandi_desktop/advanced_settings.dart';
import 'package:yandi_desktop/features.dart';
import 'package:yandi_desktop/devices.dart';
import 'package:yandi_desktop/calls.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'advanced_settings_test.dart' as a;
import 'devices_test.dart' as d;

Future<void> show(WidgetTester t, Widget child) async {
  final prefs = AppPreferences()..english = true;
  await t.pumpWidget(
    PreferencesScope(
      preferences: prefs,
      child: MaterialApp(
        home: Scaffold(body: SingleChildScrollView(child: child)),
      ),
    ),
  );
  await t.pumpAndSettle();
}

void main() {
  test(
    'all catalog entries and dynamic captions translate with names preserved',
    () {
      for (final e in englishLabels.entries) {
        expect(translate(e.key, english: true), e.value);
      }
      for (final e in englishTemplates.entries) {
        final rendered = e.key.replaceAllMapped(
          RegExp(r'\{[0-9]+\}'),
          (_) => 'PRIVATE',
        );
        expect(
          RegExp('[А-Яа-яЁё]').hasMatch(translate(rendered, english: true)),
          false,
          reason: e.key,
        );
      }
      expect(translate('Чат: Сохранить', english: true), 'Chat: Сохранить');
      expect(
        translate(const ApiFailure('timeout').message, english: true),
        contains('Retry manually'),
      );
      expect(translate('Подождите 3 с', english: true), 'Wait 3 s');
    },
  );
  testWidgets('EN group forms rules and confirmation', (t) async {
    await show(
      t,
      const GroupForm(
        title: 'Создать группу',
        labels: ['Название', 'Тематика', 'Описание'],
        creating: true,
      ),
    );
    expect(find.widgetWithText(TextField, 'Topic'), findsOneWidget);
    expect(find.text('By invitation'), findsOneWidget);
    await show(t, const GroupRules(initial: []));
    expect(find.text('Forbid Images'), findsOneWidget);
    await show(
      t,
      const GroupConfirm(
        title: 'Отозвать устройство?',
        detail: 'Токен устройства перестанет действовать.',
      ),
    );
    expect(find.text('Revoke device?'), findsOneWidget);
  });
  testWidgets('EN proxy fields, validation and message editor', (t) async {
    await show(t, const ProxyForm());
    expect(find.widgetWithText(TextField, 'Proxy password'), findsOneWidget);
    await t.tap(find.text('Save'));
    await t.pumpAndSettle();
    expect(find.textContaining('Enter a host'), findsOneWidget);
    await show(
      t,
      const ContactNameDialog(initial: 'Сохранить', editingMessage: true),
    );
    expect(find.widgetWithText(TextField, 'Message text'), findsOneWidget);
    expect(
      t.widget<TextField>(find.byType(TextField)).controller!.text,
      'Сохранить',
    );
  });
  testWidgets('EN settings, devices, about and call states', (t) async {
    await show(
      t,
      AdvancedSettings(
        api: a.SettingsApi(),
        busy: false,
        isActive: () => true,
        mutate: (f) => f(),
      ),
    );
    expect(find.text('Upstream proxies for internet exit'), findsOneWidget);
    expect(find.text('Internet exit through your node'), findsOneWidget);
    await show(
      t,
      DevicesPanel(
        api: d.DeviceApi(),
        paired: false,
        busy: false,
        isActive: () => true,
        mutate: (f) => f(),
      ),
    );
    expect(find.text('Hidden'), findsOneWidget);
    await show(t, const AboutPanel());
    expect(find.text('About'), findsOneWidget);
    await show(
      t,
      CallsFeature(
        api: d.DeviceApi(),
        isActive: () => true,
        onLocked: () {},
        onFailure: (_) {},
      ),
    );
    expect(
      find.widgetWithText(TextField, 'Peer ID (16 or 64 hex)'),
      findsOneWidget,
    );
    await t.pumpWidget(const SizedBox());
    await t.pumpAndSettle();
  });
}
