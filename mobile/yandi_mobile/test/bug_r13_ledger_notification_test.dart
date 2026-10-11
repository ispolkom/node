import 'dart:io';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/replay_ledger.dart';
import 'package:yandi_mobile/services/storage_service.dart';
import 'package:yandi_mobile/services/notification_service.dart';
import 'package:yandi_mobile/services/vault_service.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late Directory tmp;
  setUp(() {
    FlutterSecureStorage.setMockInitialValues({});
    StorageService.setSecureKey(List.filled(32, 7));
    tmp = Directory.systemTemp.createTempSync('r14_ledger_');
  });
  tearDown(() async { StorageService.setSecureKey(null); await tmp.delete(recursive: true); });
  ReplayLedger ledger(File file, {Future<void> Function()? after}) => ReplayLedger(file,
      seal: StorageService.ledgerSeal, open: StorageService.ledgerOpen, afterWrite: after ?? StorageService.ledgerSealed);

  test('bug_r13_missing_sealed_ledger_must_fail_closed', () async {
    final file = File('${tmp.path}/ledger');
    final journal = ledger(file);
    expect(await journal.claim('accepted', 5000, nowMs: 1000), isTrue);
    await file.delete();
    await expectLater(journal.claim('accepted', 5000, nowMs: 1000), throwsStateError);
    await expectLater(journal.reseal(), throwsStateError);
    expect(file.existsSync(), isFalse);
  });

  test('bug_r13_ledger_marker_write_crash_must_not_leave_downgrade_window', () async {
    final file = File('${tmp.path}/ledger')..writeAsStringSync('{"old":5000}');
    final failing = ledger(file, after: () async { throw StateError('marker write failed'); });
    await expectLater(failing.reseal(), throwsStateError);
    expect(file.readAsStringSync(), startsWith('e1:'));
    final restarted = ledger(file);
    await restarted.reseal();
    expect(await StorageService.secureRead('vault_ledger_sealed'), '1');
    expect(await restarted.claim('old', 5000, nowMs: 1000), isFalse);
    file.writeAsStringSync('{}');
    await expectLater(restarted.claim('old', 5000, nowMs: 1000), throwsStateError);
  });

  test('bug_r13_cold_start_notification_must_capture_launch_payload', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    const channel = MethodChannel('dexterous.com/flutter/local_notifications');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'getNotificationAppLaunchDetails') {
        return {'notificationLaunchedApp': true, 'notificationResponse': {
          'notificationId': 1, 'notificationResponseType': 0, 'payload': 'a' * 64,
        }};
      }
      return true;
    });
    try {
      NotificationService.pendingOpenPeerId = null;
      expect(VaultService.instance.unlocked, isFalse);
      await NotificationService.init();
      expect(NotificationService.pendingOpenPeerId, 'a' * 64);
      expect(VaultService.instance.unlocked, isFalse);
    } finally {
      NotificationService.pendingOpenPeerId = null;
      debugDefaultTargetPlatformOverride = null;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(channel, null);
    }
  });
}
