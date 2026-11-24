import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/identity.dart';
import 'package:yandi_mobile/services/storage_service.dart';

const _raw = FlutterSecureStorage();
final _k = List<int>.generate(32, (i) => i + 1);

void main() {
  setUp(() {
    FlutterSecureStorage.setMockInitialValues({});
    StorageService.setSecureKey(_k);
  });

  test('первый запуск: личность создаётся и потом читается та же', () async {
    final a = await Identity.loadOrGenerate();
    final b = await Identity.loadOrGenerate();
    expect(b.peerId, a.peerId);
  });

  test('повреждённый seed не заменяется новой личностью', () async {
    final a = await Identity.loadOrGenerate();
    final stored = (await _raw.read(key: 'id_x25519_seed'))!;
    await _raw.write(key: 'id_x25519_seed', value: '${stored.substring(0, stored.length - 6)}AAAAA=');
    StorageService.dropSecureCache();
    await expectLater(Identity.loadOrGenerate(), throwsStateError);
    expect(await _raw.read(key: 'id_ed25519_seed'), isNotNull, reason: 'прежняя запись на месте');
    expect(a.peerId, isNotEmpty);
  });

  test('один из двух seed отсутствует: ошибка, а не перезапись обоих', () async {
    await Identity.loadOrGenerate();
    final edBefore = await _raw.read(key: 'id_ed25519_seed');
    await _raw.delete(key: 'id_x25519_seed');
    StorageService.dropSecureCache();
    await expectLater(Identity.loadOrGenerate(), throwsStateError);
    expect(await _raw.read(key: 'id_ed25519_seed'), edBefore);
  });
}
