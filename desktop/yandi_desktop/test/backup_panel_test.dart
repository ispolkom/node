import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/backup_panel.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/transport/api.dart';

class _Api implements Api {
  final calls = <String>[];
  final bodies = <Map<String, dynamic>>[];
  String? dir;
  ApiFailure? configFails;
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    calls.add(path);
    if (body != null) bodies.add(Map.of(body));
    if (path == '/api/backup/status') {
      return {'dir': dir, 'counter': 3, 'last_saved_ms': 1792500000000, 'keep': 7, 'keep_manual': 5, 'last_error': null};
    }
    if (path == '/api/backup/places') {
      return {
        'places': [
          {'path': '/media/iam/FLASH', 'fs': 'vfat', 'writable': true, 'free_bytes': 8 * (1 << 30)},
          {'path': '/mnt/old', 'fs': 'ext4', 'writable': false, 'free_bytes': 100 * (1 << 20)},
        ],
      };
    }
    if (path == '/api/backup/config') {
      final fail = configFails;
      if (fail != null && !(body?['confirm_system'] == true && fail.code == 'system_disk')) throw fail;
      dir = (body!['dir'] as String).isEmpty ? null : '${body['dir']}/yandi-node';
      return {'dir': dir, 'counter': 3, 'last_saved_ms': 0, 'keep': 7, 'keep_manual': 5};
    }
    if (path == '/api/backup/export?quiet=1') return {'saved': '$dir/yandi-manual-x-000004.yandibak'};
    return {'ok': true};
  }

  @override
  void cancel() {}
}

Future<void> show(WidgetTester t, _Api api, {bool english = false, bool firstRun = false, VoidCallback? onDone}) async {
  final prefs = AppPreferences()..english = english;
  await t.pumpWidget(PreferencesScope(
    preferences: prefs,
    child: MaterialApp(home: Scaffold(body: SingleChildScrollView(child: BackupPanel(api: api, isActive: () => true, firstRun: firstRun, onDone: onDone)))),
  ));
  await t.pumpAndSettle();
}

void main() {
  testWidgets('not set up: a reminder, the candidate disks and no "save now"', (t) async {
    final api = _Api();
    await show(t, api);
    expect(find.text('Копии связей'), findsOneWidget);
    expect(find.text('Копии не настроены: выберите место.'), findsOneWidget);
    expect(find.textContaining('/media/iam/FLASH'), findsOneWidget);
    expect(find.textContaining('нет прав на запись'), findsOneWidget, reason: 'диск без прав помечен');
    expect(find.text('Сохранить копию сейчас'), findsNothing);
  });

  testWidgets('choosing a disk fills the path; saving sends it and shows the yandi-node folder', (t) async {
    final api = _Api();
    await show(t, api);
    await t.tap(find.text('Выбрать').first);
    await t.pump();
    expect(t.widget<TextField>(find.byType(TextField)).controller!.text, '/media/iam/FLASH');
    await t.tap(find.text('Сохранить место'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'dir': '/media/iam/FLASH', 'confirm_system': false});
    expect(find.textContaining('/media/iam/FLASH/yandi-node'), findsWidgets);
    expect(find.text('Сохранить копию сейчас'), findsOneWidget);
    await t.tap(find.text('Сохранить копию сейчас'));
    await t.pumpAndSettle();
    expect(find.textContaining('yandi-manual'), findsOneWidget);
  });

  testWidgets('no permission is told with the reason; the system disk needs a confirmation and then goes through', (t) async {
    final api = _Api()..configFails = const ApiFailure('backup', 400, [], false, 'no_write', 'permission', '');
    await show(t, api);
    await t.enterText(find.byType(TextField), '/root/x');
    await t.tap(find.text('Сохранить место'));
    await t.pumpAndSettle();
    expect(find.textContaining('У меня нет прав для записи в это место: доступ запрещён'), findsOneWidget);
    api.configFails = const ApiFailure('backup', 409, [], false, 'system_disk');
    await t.enterText(find.byType(TextField), '/home/iam/x');
    await t.tap(find.text('Сохранить место'));
    await t.pumpAndSettle();
    expect(find.text('Храните копии вне системного диска: при его потере пропадут и копии.'), findsOneWidget);
    expect(find.byKey(const ValueKey('confirm-system')), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('confirm-system')));
    await t.pump();
    await t.tap(find.text('Сохранить место'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'dir': '/home/iam/x', 'confirm_system': true});
    expect(api.dir, '/home/iam/x/yandi-node');
  });

  testWidgets('the first-run dialog can be skipped', (t) async {
    final api = _Api();
    var done = 0;
    await show(t, api, firstRun: true, onDone: () => done++);
    await t.tap(find.text('Пропустить (напомню позже)'));
    await t.pump();
    expect(done, 1);
    expect(api.dir, isNull, reason: 'пропуск ничего не включает');
  });

  testWidgets('English: texts, reasons and the system-disk warning', (t) async {
    final api = _Api()..configFails = const ApiFailure('backup', 400, [], false, 'no_write', 'read_only', '');
    await show(t, api, english: true);
    expect(find.text('Backups of the links'), findsOneWidget);
    expect(find.text('Backups are not set up: choose a place.'), findsOneWidget);
    await t.enterText(find.byType(TextField), '/mnt/ro');
    await t.tap(find.text('Save the place'));
    await t.pumpAndSettle();
    expect(find.text('I have no permission to write to this place: the disk is read-only'), findsOneWidget);
    expect(find.textContaining('not writable'), findsOneWidget);
  });

  test('a node answer is reduced to a code, a reason and a short sanitized detail', () {
    final f = backupFailureFrom(400, {'code': 'no_write', 'reason': 'permission', 'detail': 'x\u0007y' * 200, 'error': 'secret text'});
    expect(f.kind, 'backup');
    expect(f.code, 'no_write');
    expect(f.reason, 'permission');
    expect(f.detail.length, 200);
    expect(f.detail.contains('\u0007'), false);
    final bad = backupFailureFrom(400, {'code': '<script>', 'reason': 'whatever'});
    expect(bad.code, '');
    expect(bad.reason, '');
    expect(validApiPath('/api/backup/export?quiet=1'), true);
    expect(validApiPath('/api/backup/export?x=1'), false);
  });
}
