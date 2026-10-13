import 'dart:async';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'files_test.dart' as files;
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

class ProfileApi extends files.FileApi {
  String? avatar;
  Completer<Map<String, dynamic>>? peerPending;
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/profile/me') {
      if (body != null) {
        bodies.add(Map.of(body));
        avatar = body['avatar'] as String?;
      }
      return {
        'profile': {
          'display_name': profileName,
          'short_id': 'c' * 16,
          'avatar': avatar,
        },
      };
    }
    if (path.startsWith('/api/profile/') && peerPending != null)
      return peerPending!.future;
    return super.request(path, body: body, method: method);
  }

  @override
  Future<Map<String, dynamic>> multipart(
    String path,
    Map<String, String> fields,
    String field,
    String filename,
    dynamic bytes,
  ) async {
    avatar = '/api/avatar/${'c' * 16}';
    return super.multipart(path, fields, field, filename, bytes);
  }
}

void main() {
  testWidgets('settings avatar upload, memory preview and confirmed removal', (
    t,
  ) async {
    final temp = (await t.runAsync(
      () => Directory.systemTemp.createTemp('avatar-test-'),
    ))!;
    final file = File('${temp.path}/avatar.png');
    await t.runAsync(() => file.writeAsBytes(files.png));
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      PlatformFiles.channel,
      (call) async => call.method == 'choose' ? file.path : null,
    );
    final api = ProfileApi();
    await m2.open(t, api, 'Настройки');
    await t.enterText(m2.field('Имя профиля'), 'Несохранённое имя');
    await t.ensureVisible(find.text('Загрузить аватар'));
    await t.runAsync(() async {
      await t.tap(find.text('Загрузить аватар'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await t.pumpAndSettle();
    expect(api.payloads.single, files.png);
    expect(
      find.byWidgetPredicate((w) => w is Image && w.image is MemoryImage && w.width == 96),
      findsOneWidget,
    );
    await m1.click(t, 'Удалить аватар профиля');
    await m1.click(t, 'Отмена');
    expect(api.bodies.where((b) => b['avatar'] == ''), isEmpty);
    await m1.click(t, 'Удалить аватар профиля');
    await m1.click(t, 'Удалить аватар');
    expect(api.bodies.last, {'avatar': ''});
    expect(
      find.byWidgetPredicate((w) => w is Image && w.image is MemoryImage && w.width == 96),
      findsNothing,
    );
    expect(find.text('Несохранённое имя'), findsOneWidget);
    await m1.stop(t);
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      PlatformFiles.channel,
      null,
    );
    await t.runAsync(() => temp.delete(recursive: true));
  });
  testWidgets('chat peer profile renders text and discards late responses', (
    t,
  ) async {
    final api = ProfileApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    expect(find.text('Профиль собеседника'), findsOneWidget);
    await m1.click(t, 'Просмотреть аватар');
    expect(
      find.byWidgetPredicate((w) => w is Image && w.image is MemoryImage && w.width == 96),
      findsOneWidget,
    );
    api.peerPending = Completer();
    await t.ensureVisible(find.text('Обновить профиль'));
    await t.tap(find.text('Обновить профиль'));
    await t.pump();
    await m1.click(t, 'Главная');
    api.peerPending!.complete({
      'profile': {'display_name': 'late-secret'},
    });
    await t.pumpAndSettle();
    expect(find.text('late-secret'), findsNothing);
    await m1.stop(t);
  });
}
