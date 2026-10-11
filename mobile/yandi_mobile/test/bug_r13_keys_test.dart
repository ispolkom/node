import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';

class _Backend extends MemoryVaultBackend {
  int reads = 0;
  bool hold = false;
  final reached = Completer<void>(), release = Completer<void>();
  @override
  Future<String?> secureRead(String key) async {
    if (hold && key == 'vault_hw' && ++reads == 2) {
      reached.complete();
      await release.future;
    }
    return super.secureRead(key);
  }
}

void main() {
  test('bug_r13_missing_counters_must_gate_before_kdf', () async {
    final b = _Backend();
    (await VaultKeys.create(b, password: 'old-password', pwKdf: VaultKdf.test)).lock();
    b.secure.remove('vault_fail_pw');
    b.secure.remove('vault_next_pw');
    await expectLater(VaultKeys.unlock(b, 'old-password'), throwsA(isA<VaultThrottled>()));
    expect(b.reads, 0);
    b.now += 900001;
    (await VaultKeys.unlock(b, 'old-password')).lock();
  });

  test('bug_r13_password_change_during_lock_must_not_wrap_zero_data_key', () async {
    final b = _Backend();
    final k = await VaultKeys.create(b, password: 'old-password', pwKdf: VaultKdf.test);
    final key = await k.subKey('db');
    final blob = b.blob;
    b.hold = true;
    final changing = k.changePassword('old-password', 'new-password');
    final assertion = expectLater(changing, throwsStateError);
    await b.reached.future;
    k.lock();
    b.release.complete();
    await assertion;
    expect(b.blob, blob);
    final reopened = await VaultKeys.unlock(b, 'old-password');
    expect(await reopened.subKey('db'), key);
    reopened.lock();
  });
}
