import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/app.dart';
import 'package:yandi_desktop/theme.dart';
import '../tool/design_fixture.dart';
import 'widget_test.dart' show click, stop;

void main() {
  setUpAll(() async {
    // The same system font used by fontconfig on the Linux preview host.
    final loader = FontLoader('Tahoma');
    loader.addFont(
      Future.value(
        ByteData.sublistView(
          File(
            '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',
          ).readAsBytesSync(),
        ),
      ),
    );
    await loader.load();
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
  for (final size in [const Size(1280, 800), const Size(900, 600)]) {
    for (final scene in [
      'offline',
      'login',
      'setup',
      'unlock',
      'Главная',
      'Чат',
      'Группы',
      'Шлюзы',
      'Настройки',
    ]) {
      testWidgets('$scene ${size.width.toInt()}x${size.height.toInt()}', (
        t,
      ) async {
        t.view.physicalSize = size;
        t.view.devicePixelRatio = 1;
        addTearDown(t.view.resetPhysicalSize);
        addTearDown(t.view.resetDevicePixelRatio);
        await t.binding.setSurfaceSize(size);
        addTearDown(() => t.binding.setSurfaceSize(null));
        await t.pumpWidget(DesktopApp(api: DesignApi(scene)));
        await t.pumpAndSettle();
        if (!['offline', 'login', 'setup'].contains(scene)) {
          await click(t, 'Войти');
          if (scene == 'unlock')
            await click(t, 'Закрытая среда');
          else if (scene != 'Главная')
            await click(t, scene);
          if (scene == 'Чат') await click(t, 'Открыть чат');
          if (scene == 'Группы') await click(t, 'Семья · владелец');
        }
        expect(
          t.takeException(),
          isNull,
          reason: 'No overflow or rendering errors',
        );
        expect(find.byType(NavigationRail), findsNothing);
        expect(
          Theme.of(t.element(find.byType(Scaffold))).scaffoldBackgroundColor,
          Icq.background,
        );
        final key =
            '${scene.replaceAll(' ', '_')}_${size.width.toInt()}x${size.height.toInt()}';
        await expectLater(
          find.byType(Scaffold),
          matchesGoldenFile('goldens/$key.png'),
        );
        await stop(t);
      });
    }
  }
}
