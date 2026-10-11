import 'dart:async';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';
import 'package:yandi_mobile/services/storage_service.dart';
import 'package:yandi_mobile/services/vault_service.dart';

class _DelayedBackend extends MemoryVaultBackend {
  bool hold = false;
  final reached = Completer<void>(), release = Completer<void>();
  @override
  Future<String?> secureRead(String key) async {
    if (hold && key == 'vault_hw') {
      hold = false;
      reached.complete();
      await release.future;
    }
    return super.secureRead(key);
  }
}

void main() {
  final service = VaultService.instance;
  late _DelayedBackend backend;
  var opened = false;
  setUp(() async {
    FlutterSecureStorage.setMockInitialValues({});
    await service.lock();
    backend = _DelayedBackend();
    service.backend = backend;
    (await VaultKeys.create(backend, password: '123456', pwKdf: VaultKdf.test)).lock();
    opened = false;
    StorageService.debugOpen = (_) async { opened = true; };
    StorageService.debugClose = () async { opened = false; };
  });
  tearDown(() async {
    await service.lock();
    StorageService.debugOpen = null;
    StorageService.debugClose = null;
  });

  test('bug_r13_lock_during_password_derivation_must_cancel_unlock', () async {
    backend.hold = true;
    final opening = service.unlock('123456');
    final cancelled = expectLater(opening, throwsStateError);
    await backend.reached.future;
    expect(service.active, isTrue);
    await service.lock();
    backend.release.complete();
    await cancelled;
    expect(service.unlocked, isFalse);
    expect(opened, isFalse);
    await service.unlock('123456');
    expect(opened, isTrue);
  });

  test('bug_r13_cancelled_old_open_must_not_close_new_session_database', () async {
    final reached = Completer<void>(), release = Completer<void>();
    var calls = 0;
    StorageService.debugOpen = (_) async {
      if (++calls == 1) {
        reached.complete();
        await release.future;
      }
      opened = true;
    };
    final old = service.unlock('123456');
    final cancelled = expectLater(old, throwsStateError);
    await reached.future;
    await service.lock();
    final next = service.unlock('123456');
    // Новый вход ждёт окончания старого открытия и его отката; ключи старого уже отозваны.
    release.complete();
    await cancelled;
    await next;
    expect(service.unlocked, isTrue);
    expect(opened, isTrue);
    expect(calls, 2);
  });

  test('bug_r13_reopening_live_service_must_revoke_previous_keys_and_files', () async {
    await service.unlock('123456');
    final oldFiles = await service.files();
    await service.unlock('123456');
    final nextFiles = await service.files();
    expect(oldFiles.disposed, isTrue);
    expect(identical(oldFiles, nextFiles), isFalse);
    await service.lock();
    expect(nextFiles.disposed, isTrue);
  });
}
