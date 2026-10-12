import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/group_repository.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'groups_test.dart' as g;
import 'files_test.dart' as f;
import 'widget_test.dart' as m1;
class GroupFiles extends g.GroupApi implements BinaryApi, GroupFileApi {
  String? uploaded;
  bool reject = false;
  @override
  Future<Map<String, dynamic>> uploadGroup(String path, File file, {required bool Function() isActive, required void Function(int, int) progress}) async {
    uploaded = path;
    if (reject) throw const ApiFailure('http', 400);
    progress(3, 3);
    return {'seq': 6};
  }
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    if (path == '/api/groups/devices') return {'devices': [{'id': 'd' * 64, 'name': 'Picker phone'}]};
    return super.request(path, body: body, method: method);
  }
  @override
  Future<Uint8List> bytes(String path, {int limit = 8 * 1024 * 1024}) async => Uint8List.fromList(f.png);
  @override
  Future<void> download(String path, File destination, {required int limit, required bool Function() isActive}) async { if (isActive()) await destination.writeAsBytes(f.png); }
  @override
  Future<Map<String, dynamic>> multipart(String path, Map<String, String> fields, String field, String filename, Uint8List bytes) async => {};
}
void main() {
  test('group display whitelist discards keys and accepts metadata only', () async {
    final api = GroupFiles();
    api.messages.last['body'] = {'n': 'picture.png', 'z': 3, 'k': 'SECRET', 'h': 'HASH', 'f': 'FID'};
    final feed = await GroupRepository(api).feed(g.gid);
    expect(feed.toString(), isNot(contains('SECRET')));
    expect(feed.toString(), isNot(contains('HASH')));
    expect((feed['messages'] as List).last['attachment']['filename'], 'picture.png');
  });
  testWidgets('group preview and phone picker use group endpoints', (t) async {
    final api = GroupFiles();
    api.messages.last['body'] = {'n': 'picture.png', 'z': f.png.length, 'k': 'SECRET'};
    await g.open(t, api, conversation: true);
    await m1.click(t, 'Предпросмотр');
    await m1.click(t, 'Пригласить телефон');
    await t.tap(find.text('Picker phone'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'peer': 'd' * 64});
    await m1.stop(t);
  });
  testWidgets('group upload preserves selection after rejection', (t) async {
    final api = GroupFiles()..reject = true;
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(PlatformFiles.channel, (call) async => '/tmp/group file.bin');
    addTearDown(() => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(PlatformFiles.channel, null));
    await g.open(t, api, conversation: true);
    await m1.click(t, 'Выбрать файл');
    await m1.click(t, 'Отправить файл');
    expect(api.uploaded, '/api/groups/${g.gid}/upload?name=group%20file.bin');
    expect(find.text('group file.bin'), findsOneWidget);
    await m1.stop(t);
  });
}
