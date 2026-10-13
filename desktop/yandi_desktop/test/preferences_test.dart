import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/preferences.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'notifications baseline, privacy, archive, own messages, DND and reset',
    () async {
      final alerts = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(PlatformFiles.channel, (call) async {
            alerts.add(call);
            return null;
          });
      final prefs = AppPreferences()
        ..sound = true
        ..notifications = true;
      final rows = <Map<String, dynamic>>[
        {
          'msg_id': 'archive',
          'from': 'peer',
          'timestamp': 100,
          'text': 'private text',
        },
      ];
      await prefs.observe('peer', 'me', rows);
      expect(alerts, isEmpty);
      rows.add({
        'msg_id': 'new',
        'from': 'peer',
        'timestamp': 101,
        'text': 'secret',
      });
      await prefs.observe('peer', 'me', rows);
      expect(alerts.single.arguments, {
        'sound': true,
        'notify': true,
        'english': false,
      });
      await prefs.observe('peer', 'me', rows);
      expect(alerts.length, 1);
      rows.add({'msg_id': 'historical', 'from': 'peer', 'timestamp': 10});
      rows.add({'msg_id': 'own', 'from': 'me', 'timestamp': 102});
      await prefs.observe('peer', 'me', rows);
      expect(alerts.length, 1);
      prefs.dnd = true;
      rows.add({'msg_id': 'dnd', 'from': 'peer', 'timestamp': 103});
      await prefs.observe('peer', 'me', rows);
      expect(alerts.length, 1);
      prefs.dnd = false;
      await prefs.observe('peer', 'me', rows);
      expect(alerts.length, 1);
      prefs.reset();
      await prefs.observe('peer', 'me', rows);
      expect(alerts.last.method, 'withdrawNotification');
      await prefs.observe('another-peer', 'me', rows);
      expect(alerts.length, 2);
      prefs.dispose();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(PlatformFiles.channel, null);
    },
  );
  testWidgets('RU/EN settings and opt-in sound, OS notifications and DND', (
    t,
  ) async {
    final api = m2.FeatureApi();
    await m2.open(t, api, 'Настройки');
    final sound = find.widgetWithText(CheckboxListTile, 'Системный звук');
    expect(t.widget<CheckboxListTile>(sound).value, false);
    await t.ensureVisible(sound);
    await t.tap(sound);
    await t.pumpAndSettle();
    expect(t.widget<CheckboxListTile>(sound).value, true);
    final language = find.widgetWithText(CheckboxListTile, 'Язык / Language');
    await t.ensureVisible(language);
    await t.tap(language);
    await t.pumpAndSettle();
    expect(find.text('Contacts'), findsNothing, reason: 'вкладки «Контакты» нет');
    expect(find.text('Chat'), findsOneWidget);
    expect(find.text('System sound'), findsOneWidget);
    expect(
      find.textContaining('Language applies to all shell forms and states'),
      findsOneWidget,
    );
    await m1.click(t, 'Chat');
    await m1.click(t, 'Add');
    expect(find.text('Export contacts.json'), findsOneWidget);
    await m1.stop(t);
  });
}
