import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/services/vault_files.dart';

final _root = List<int>.generate(32, (i) => i + 1);

/// Независимая запись прежнего формата v1: ключ не зависит от имени файла.
Future<void> _legacy(File file, List<int> plain) async {
  final aes = AesGcm.with256bits();
  final id = List<int>.filled(16, 7), prefix = [1, 2, 3, 4];
  final key = await Hkdf(hmac: Hmac.sha256(), outputLength: 32).deriveKey(
      secretKey: SecretKey(_root), nonce: id, info: utf8.encode('yandi-file-key:v1'));
  final meta = utf8.encode(jsonEncode({'n': 'old.txt', 'm': 'text/plain', 's': plain.length, 't': 42}));
  final header = [89, 70, 69, 49, 1, 0, 0, 0, ...id, ...prefix,
    ...(ByteData(4)..setUint32(0, 1024)).buffer.asUint8List(),
    ...(ByteData(2)..setUint16(0, meta.length + 28)).buffer.asUint8List()];
  final metadata = await aes.encrypt(meta, secretKey: key, nonce: List.filled(12, 8), aad: [...header, ...utf8.encode('meta')]);
  final out = BytesBuilder()..add(header)..add(metadata.concatenation());
  for (var offset = 0, index = 0; offset < plain.length; offset += 1024, index++) {
    final end = offset + 1024 < plain.length ? offset + 1024 : plain.length;
    final position = (ByteData(8)..setUint64(0, index)).buffer.asUint8List();
    final box = await aes.encrypt(plain.sublist(offset, end), secretKey: key,
        nonce: [...prefix, ...position], aad: [...header, ...position, end == plain.length ? 1 : 0]);
    out.add([...box.cipherText, ...box.mac.bytes]);
  }
  await file.writeAsBytes(out.takeBytes(), flush: true);
}

void main() {
  late Directory tmp;
  late FileVault vault;
  setUp(() { tmp = Directory.systemTemp.createTempSync('r14_files_'); vault = FileVault.withKey(_root); });
  tearDown(() async { vault.dispose(); await tmp.delete(recursive: true); });

  test('bug_r13_pre_upgrade_v1_file_must_remain_readable', () async {
    final file = File('${tmp.path}/legacy.yfe');
    final data = List<int>.generate(2300, (i) => i % 251);
    await _legacy(file, data);
    expect(await vault.readAll(file), data);
    final files = VaultFiles(tmp, vault: () async => vault);
    expect(await files.migrateLegacy(), 1);
    expect(file.readAsBytesSync()[4], 2);
    expect(await vault.readAll(file), data);
    expect((await vault.readMeta(file)).createdMs, 42);
    expect(await files.migrateLegacy(), 0);
    final substituted = File('${tmp.path}/other.yfe')..writeAsBytesSync(file.readAsBytesSync());
    await expectLater(vault.readAll(substituted), throwsA(isA<FileVaultException>()));
  });

  test('повреждённый v1 не заменяется при неудаче переноса', () async {
    final file = File('${tmp.path}/legacy.yfe');
    await _legacy(file, List.filled(2300, 7));
    final bytes = file.readAsBytesSync()..[file.lengthSync() - 1] ^= 1;
    file.writeAsBytesSync(bytes);
    final files = VaultFiles(tmp, vault: () async => vault);
    await files.migrateLegacy();
    expect(files.migrationFailures, 1);
    expect(file.readAsBytesSync(), bytes);
  });

  test('bug_r13_legacy_retry_must_verify_chunks_before_wiping_only_good_copy', () async {
    final source = File('${tmp.path}/abcdef01_old.txt')..writeAsStringSync('precious');
    var allow = false;
    final files = VaultFiles(tmp, vault: () async => vault, wipe: (path) async {
      if (!allow) return false;
      await File(path).delete();
      return true;
    });
    await files.migrateLegacy();
    final target = File((await files.list()).single.path);
    final bad = target.readAsBytesSync()..[target.lengthSync() - 1] ^= 1;
    target.writeAsBytesSync(bad);
    allow = true;
    await files.migrateLegacy();
    expect(files.migrationFailures, 0);
    expect(source.existsSync(), isFalse);
    expect(utf8.decode(await vault.readAll(target)), 'precious');
  });
}
