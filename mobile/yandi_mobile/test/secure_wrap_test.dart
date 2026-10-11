import 'dart:io';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:yandi_mobile/crypto/replay_ledger.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/storage_service.dart';

const _raw = FlutterSecureStorage();
final _k1 = List<int>.generate(32, (i) => i + 1);
final _k2 = List<int>.generate(32, (i) => 200 - i);

void main() {
  setUp(() {
    FlutterSecureStorage.setMockInitialValues({});
    StorageService.setSecureKey(null);
  });

  test('значения хранятся зашифрованными, читаются тем же ключом, чужим ключом — нет', () async {
    StorageService.setSecureKey(_k1);
    await StorageService.secureWrite('id_ed25519_seed', 'очень секретный seed');
    final stored = await _raw.read(key: 'id_ed25519_seed');
    expect(stored, startsWith('e1:'));
    expect(stored, isNot(contains('seed')));
    expect(await StorageService.secureRead('id_ed25519_seed'), 'очень секретный seed');

    StorageService.setSecureKey(_k2);
    expect(await StorageService.secureRead('id_ed25519_seed'), isNull, reason: 'другой ключ данных — прочесть нечем');
  });

  test('значение привязано к имени ключа: подмена между ключами и порча отвергаются', () async {
    StorageService.setSecureKey(_k1);
    await StorageService.secureWrite('node_token_a', 'токен A');
    await StorageService.secureWrite('node_token_b', 'токен B');
    await _raw.write(key: 'node_token_b', value: (await _raw.read(key: 'node_token_a'))!);   // значение A под именем B
    expect(await StorageService.secureRead('node_token_b'), isNull);
    final a = (await _raw.read(key: 'node_token_a'))!;
    await _raw.write(key: 'node_token_a', value: '${a.substring(0, a.length - 6)}AAAAA=');
    expect(await StorageService.secureRead('node_token_a'), isNull, reason: 'повреждённое значение не принимается');
  });

  test('пока заблокировано: читать нечем, писать нельзя', () async {
    StorageService.setSecureKey(_k1);
    await StorageService.secureWrite('x', 'значение');
    StorageService.setSecureKey(null);
    expect(await StorageService.secureRead('x'), isNull);
    await expectLater(StorageService.secureWrite('y', 'z'), throwsStateError);
    expect((await _raw.read(key: 'y')), isNull, reason: 'ничего не записано открытым');
  });

  test('служебные записи хранилища ключей (vault_…) остаются как есть: без них пароль не проверить', () async {
    await StorageService.secureWrite('vault_hw', 'секрет-keystore');
    expect(await _raw.read(key: 'vault_hw'), 'секрет-keystore');
    expect(await StorageService.secureRead('vault_hw'), 'секрет-keystore', reason: 'и в заблокированном состоянии');
  });

  test('перенос: старые открытые значения заворачиваются один раз, потом открытые не принимаются', () async {
    FlutterSecureStorage.setMockInitialValues({'id_x25519_seed': 'старый открытый', 'vault_fail_pw': '2', 'pref_secure_keyboard': '1'});
    StorageService.setSecureKey(_k1);
    expect(await StorageService.secureRead('id_x25519_seed'), 'старый открытый', reason: 'до переноса читается и открытое');
    await StorageService.wrapExistingSecureValues();
    expect(await _raw.read(key: 'id_x25519_seed'), startsWith('e1:'));
    expect(await _raw.read(key: 'pref_secure_keyboard'), startsWith('e1:'));
    expect(await _raw.read(key: 'vault_fail_pw'), '2', reason: 'служебное не трогаем');
    expect(await StorageService.secureRead('id_x25519_seed'), 'старый открытый');

    // после переноса подброшенное открытое значение не принимается
    await _raw.write(key: 'node_token_evil', value: 'подложный-токен');
    expect(await StorageService.secureRead('node_token_evil'), isNull);
    await StorageService.warmSecureCache();
    expect(await StorageService.secureRead('node_token_evil'), isNull);
    expect(await StorageService.secureRead('id_x25519_seed'), 'старый открытый');
    // повторный перенос ничего не ломает
    await StorageService.wrapExistingSecureValues();
    expect(await StorageService.secureRead('id_x25519_seed'), 'старый открытый');
  });

  test('кеш при запуске: расшифрованные значения в памяти, после блокировки их там нет', () async {
    StorageService.setSecureKey(_k1);
    await StorageService.secureWrite('a', '1');
    await StorageService.secureWrite('b', '2');
    await StorageService.warmSecureCache();
    expect(await StorageService.secureRead('a'), '1');
    expect((await StorageService.secureKeys()).toSet(), {'a', 'b'});
    StorageService.setSecureKey(null);
    expect(await StorageService.secureRead('a'), isNull);
  });

  test('журнал повторов шифруется ключом данных: на диске нет номеров, порча и чужой ключ отвергаются, старый открытый читается', () async {
    final dir = Directory.systemTemp.createTempSync('ledger');
    try {
      final f = File('${dir.path}/l.json');
      final soon = DateTime.now().millisecondsSinceEpoch + 100000;
      ReplayLedger mk() => ReplayLedger(f, seal: StorageService.ledgerSeal, open: StorageService.ledgerOpen);
      StorageService.setSecureKey(_k1);
      expect(await mk().claim('peerA|peerB|rnd1', soon), isTrue);
      expect(f.readAsStringSync(), startsWith('e1:'));
      expect(f.readAsStringSync(), isNot(contains('peerA')));
      expect(await mk().claim('peerA|peerB|rnd1', soon), isFalse, reason: 'повтор виден');

      StorageService.setSecureKey(_k2);
      await expectLater(mk().claim('x', soon), throwsStateError, reason: 'чужой ключ: журнал не сбрасывается, а отвергается');
      StorageService.setSecureKey(_k1);
      final good = f.readAsStringSync();
      f.writeAsStringSync('${good.substring(0, good.length - 6)}AAAAA=');
      await expectLater(mk().claim('y', soon), throwsStateError, reason: 'порча');

      // старый открытый журнал (до включения защиты) читается и при первой записи шифруется
      f.writeAsStringSync('{"old|id|1": ${soon + 50000}}');
      expect(await mk().claim('new|id|2', soon), isTrue);
      expect(f.readAsStringSync(), startsWith('e1:'));
      expect(await mk().claim('old|id|1', soon), isFalse, reason: 'старая запись не потерялась');

      // после того как журнал записан зашифрованным, подброшенный открытый вид (например, пустой) не принимается (R12-11)
      final marked = ReplayLedger(f, seal: StorageService.ledgerSeal, open: StorageService.ledgerOpen, afterWrite: StorageService.ledgerSealed);
      expect(await marked.claim('mark|id|1', soon), isTrue);
      final goodBytes = f.readAsStringSync();
      f.writeAsStringSync('{}');
      await expectLater(marked.claim('mark|id|1', soon), throwsStateError, reason: 'старая запись не должна стать новой');
      f.writeAsStringSync(goodBytes);
      expect(await marked.claim('mark|id|1', soon), isFalse);
      await _raw.delete(key: 'vault_ledger_sealed');

      // и без новых сообщений: при входе старый открытый журнал шифруется сразу
      f.writeAsStringSync('{"legacy|id|9": ${soon + 50000}}');
      await mk().reseal();
      expect(f.readAsStringSync(), startsWith('e1:'));
      expect(await mk().claim('legacy|id|9', soon), isFalse, reason: 'запись не потерялась');
      final sealed = f.readAsStringSync();
      await mk().reseal();   // уже зашифрован — не трогается
      expect(f.readAsStringSync(), sealed);
    } finally {
      dir.deleteSync(recursive: true);
    }
  });
}
