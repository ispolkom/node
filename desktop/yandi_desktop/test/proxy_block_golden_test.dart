import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/advanced_settings.dart';
import 'package:yandi_desktop/theme.dart';
import 'advanced_settings_test.dart' show SettingsApi;
import 'widget_test.dart' show stop;

void main() {
  setUpAll(() async {
    final sans = FontLoader('YandiSans');
    for (final f in ['Regular', 'Italic', 'Bold', 'BoldItalic']) {
      sans.addFont(
        Future.value(
          ByteData.sublistView(File('assets/fonts/LiberationSans-$f.ttf').readAsBytesSync()),
        ),
      );
    }
    await sans.load();
    final icons = FontLoader('MaterialIcons');
    icons.addFont(
      Future.value(
        ByteData.sublistView(
          File(
            '${File(Platform.resolvedExecutable).parent.parent.parent.path}/material_fonts/MaterialIcons-Regular.otf',
          ).readAsBytesSync(),
        ),
      ),
    );
    await icons.load();
  });
  for (final size in [const Size(1280, 700), const Size(900, 700)]) {
    testWidgets('Блок внешних прокси ${size.width.toInt()}', (t) async {
      t.view.physicalSize = size;
      t.view.devicePixelRatio = 1;
      addTearDown(t.view.resetPhysicalSize);
      addTearDown(t.view.resetDevicePixelRatio);
      await t.pumpWidget(
        MaterialApp(
          theme: Icq.solidButtons(Icq.theme),
          home: Scaffold(
            body: SingleChildScrollView(
              padding: const EdgeInsets.all(12),
              child: AdvancedSettings(
                api: SettingsApi(),
                busy: false,
                isActive: () => true,
                mutate: (a) => a(),
              ),
            ),
          ),
        ),
      );
      await t.pumpAndSettle();
      await t.tap(find.byIcon(Icons.visibility));
      await t.tap(find.byType(PanelCheck).at(1));
      await t.pumpAndSettle();
      expect(t.takeException(), isNull);
      await expectLater(
        find.byType(Scaffold),
        matchesGoldenFile('goldens/Прокси_${size.width.toInt()}.png'),
      );
      await stop(t);
    });
  }
}
