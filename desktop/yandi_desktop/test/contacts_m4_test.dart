import 'dart:io';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'files_test.dart' as files;
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

class ContactsApi extends files.FileApi {
  bool trusted = true, tunnel = false, rejectTunnel = false;
  String? exported;
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/peers/trusted/remove') {
      calls.add(path);
      bodies.add(body!);
      trusted = false;
      return {'ok': true};
    }
    if (path == '/api/peers/trusted' && body == null)
      return {
        'card': 'YANDI-NODE-1:own',
        'peers': [
          if (trusted) {'id': m2.alice},
          {'id': m2.bob},
        ],
      };
    if (path.startsWith('/api/tunnel/')) {
      calls.add(path);
      if (body != null) bodies.add(body);
      if (rejectTunnel) throw const ApiFailure('business');
      if (path.contains('/start/')) {
        tunnel = true;
        return {'status': 'success'};
      }
      if (path.contains('/stop/')) {
        tunnel = false;
        return {'status': 'success'};
      }
      if (!tunnel) throw const ApiFailure('business');
      return {
        'tunnel': {
          'tunnel_status': 'Active',
          'tunnel_type': 'Generic',
          'bytes_sent': 4,
          'bytes_received': 8,
        },
      };
    }
    return super.request(path, body: body, method: method);
  }

  @override
  Future<void> download(
    String path,
    File destination, {
    required int limit,
    required bool Function() isActive,
  }) async {
    if (isActive()) {
      exported = path;
      await destination.writeAsString('{"contacts":[]}');
    }
  }
}

void main() {
  testWidgets(
    'own card copy, chosen export and trust revocation preserve contact',
    (t) async {
      final temp = (await t.runAsync(
        () => Directory.systemTemp.createTemp('export-test-'),
      ))!;
      final target = File('${temp.path}/contacts.json');
      String? copied;
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData')
            copied = (call.arguments as Map)['text'] as String;
          return null;
        },
      );
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        PlatformFiles.channel,
        (call) async => call.method == 'choose' ? target.path : null,
      );
      final api = ContactsApi();
      await m2.open(t, api, 'Контакты');
      await m1.click(t, 'Копировать карточку');
      expect(copied, 'YANDI-NODE-1:own');
      await t.ensureVisible(find.text('Экспорт contacts.json'));
      await t.runAsync(() async {
        await t.tap(find.text('Экспорт contacts.json'));
        await Future<void>.delayed(const Duration(milliseconds: 100));
      });
      await t.pumpAndSettle();
      expect(api.exported, '/api/contacts/export');
      expect(await t.runAsync(target.readAsString), '{"contacts":[]}');
      await m1.click(t, 'Отозвать доверие');
      await m1.click(t, 'Отмена');
      expect(api.trusted, true);
      await m1.click(t, 'Отозвать доверие');
      // Confirm button shares title; use the button finder.
      await m1.click(t, 'Подтвердить');
      expect(api.trusted, false);
      expect(api.rows.length, 2);
      expect(api.bodies.last, {'id': m2.alice});
      await m1.stop(t);
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        PlatformFiles.channel,
        null,
      );
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      );
      await t.runAsync(() => temp.delete(recursive: true));
    },
  );
  testWidgets(
    'tunnel start polls daemon, stop and failure never report active',
    (t) async {
      final api = ContactsApi();
      await m2.open(t, api, 'Контакты');
      await m1.click(t, 'Открыть чат');
      expect(
        find.textContaining('Нет подтверждённого туннеля'),
        findsOneWidget,
      );
      await m1.click(t, 'Запустить туннель');
      expect(find.textContaining('Туннель: Active'), findsOneWidget);
      expect(api.bodies.last, {'type': 'generic'});
      await m1.click(t, 'Остановить туннель');
      expect(find.textContaining('Туннель: Active'), findsNothing);
      api.rejectTunnel = true;
      await m1.click(t, 'Запустить туннель');
      expect(find.textContaining('Демон отклонил действие'), findsOneWidget);
      expect(find.textContaining('Туннель: Active'), findsNothing);
      await m1.stop(t);
    },
  );
}
