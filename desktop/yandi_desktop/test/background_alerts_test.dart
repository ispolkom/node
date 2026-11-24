import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/background_alerts.dart';
import 'package:yandi_desktop/preferences.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'shell_m2_test.dart' as m2;
import 'devices_test.dart' as d;

class BackgroundApi extends d.DeviceApi {
  final groupMessages = <Map<String, dynamic>>[];
  final group = {'id': 'e' * 32, 'role': 'owner'};
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/mobile/devices') throw StateError('403 admin_required');
    if (path == '/api/groups')
      return {
        'groups': [group],
      };
    if (path.startsWith('/api/groups/'))
      return {
        'group': group,
        'has_key': true,
        'deny': <int>[],
        'messages': groupMessages,
      };
    if (path.startsWith('/api/chat/history/')) {
      calls.add(path);
      return {'messages': historyRows.map(Map<String, dynamic>.of).toList()};
    }
    return super.request(path, body: body, method: method);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test('background polling without admin does not reset the open chat sound baseline', () async {
    final api = BackgroundApi();
    final prefs = AppPreferences();
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(PlatformFiles.channel, (call) async { calls.add(call); return null; });
    final alerts = BackgroundAlerts(api, prefs, () => true);
    addTearDown(() {
      alerts.dispose(); prefs.dispose();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(PlatformFiles.channel, null);
    });
    await prefs.observe(m2.alice, 'b' * 64, []);
    await alerts.poll();
    await prefs.observe(m2.alice, 'b' * 64, [
      {'msg_id': 'fresh', 'from': m2.alice, 'text': 'new', 'timestamp': 1},
    ]);
    expect(calls.single.method, 'messageAlert');
  });
  test('new phone message alerts outside the selected chat, including a short peer ID', () async {
    final api = BackgroundApi();
    final phone = 'd' * 64;
    final short = phone.substring(0, 16);
    api.rows.add({'id': 'phone', 'short_id': short, 'is_phone': true});
    final prefs = AppPreferences();
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(PlatformFiles.channel, (call) async { calls.add(call); return null; });
    final alerts = BackgroundAlerts(api, prefs, () => true);
    addTearDown(() {
      alerts.dispose();
      prefs.dispose();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(PlatformFiles.channel, null);
    });
    await alerts.poll();
    expect(calls, isEmpty);
    api.historyRows.add({'msg_id': 'phone-new', 'from': phone,
      'timestamp': DateTime.now().millisecondsSinceEpoch, 'text': 'message'});
    await alerts.poll();
    expect(calls.single.method, 'messageAlert');
    expect(calls.single.arguments['sound'], isTrue);
    expect(alerts.unread, contains(short));
    await alerts.poll();
    expect(calls.length, 1);
  });
  test(
    'background baseline, phones included, privacy, DND, lock and reset',
    () async {
      final api = BackgroundApi();
      api.rows.add({
        'id': 'phone',
        'short_id': 'd' * 64,
        'is_phone': true,
        'name': 'PRIVATE',
      });
      final prefs = AppPreferences()
        ..background = true
        ..notifications = true;
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(PlatformFiles.channel, (call) async {
            calls.add(call);
            return null;
          });
      final alerts = BackgroundAlerts(api, prefs, () => true);
      addTearDown(() {
        alerts.dispose();
        prefs.dispose();
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(PlatformFiles.channel, null);
      });
      await alerts.poll();
      expect(calls, isEmpty);
      api.historyRows.add({
        'msg_id': 'incoming',
        'from': m2.alice,
        'timestamp': DateTime.now().millisecondsSinceEpoch,
        'text': 'PRIVATE',
      });
      await alerts.poll();
      expect(calls.single.arguments, {
        'sound': true,
        'notify': true,
        'english': false,
      });
      expect(alerts.unread, contains(m2.alice));
      expect(
        api.calls.where((c) => c.contains('/history/') && c.endsWith('d' * 64)),
        isNotEmpty,
      );
      await alerts.poll();
      expect(calls.length, 1);
      alerts.opened(m2.alice);
      expect(alerts.unread, isEmpty);
      prefs.dnd = true;
      alerts.openPeer = null;
      api.historyRows.add({
        'msg_id': 'dnd',
        'from': m2.alice,
        'timestamp': DateTime.now().millisecondsSinceEpoch,
      });
      await alerts.poll();
      expect(calls.length, 1);
      prefs.dnd = false;
      api.groupMessages.add({
        'seq': 1,
        'ts': DateTime.now().millisecondsSinceEpoch,
        'kind': 1,
        'mine': false,
        'body': {'t': 'GROUP PRIVATE'},
      });
      await alerts.poll();
      expect(calls.length, 2);
      expect(alerts.unread, contains('group:${'e' * 32}'));
      alerts.opened('group:${'e' * 32}');
      expect(alerts.unread.contains('group:${'e' * 32}'), false);
      api.configured = true;
      api.unlocked = false;
      await alerts.poll();
      expect(alerts.baseline, isEmpty);
      expect(alerts.unread, isEmpty);
      await Future<void>.delayed(Duration.zero);
      expect(calls.last.method, 'withdrawNotification');
    },
  );
}
