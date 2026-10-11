import 'dart:async';
import 'dart:io';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';
import 'package:yandi_mobile/services/storage_service.dart';
import 'package:yandi_mobile/services/vault_bytes_cache.dart';
import 'package:yandi_mobile/services/vault_service.dart';

/// Жизненный цикл закрытой среды: параллельные открытия и блокировки, сбои закрытия, отзыв ключей у уже начатых операций.
void main() {
  final v = VaultService.instance;
  late Directory tmp;
  var opens = 0, closes = 0;

  setUp(() async {
    FlutterSecureStorage.setMockInitialValues({});
    v.backend = MemoryVaultBackend();
    await v.lock();
    tmp = Directory.systemTemp.createTempSync('vsvc');
    opens = 0;
    closes = 0;
    StorageService.debugOpen = (key) async => opens++;
    StorageService.debugClose = () async => closes++;
  });
  tearDown(() async {
    await v.lock();
    StorageService.debugOpen = null;
    StorageService.debugClose = null;
    tmp.deleteSync(recursive: true);
  });

  Future<void> unlocked() async {
    await v.create('пароль-тест', null);
    await v.unlock('пароль-тест');
  }

  test('files(): параллельные вызовы получают один объект, блокировка отзывает его у всех (R12-6)', () async {
    await unlocked();
    final both = await Future.wait([v.files(), v.files(), v.files()]);
    expect(identical(both[0], both[1]) && identical(both[1], both[2]), isTrue);
    final f = File('${tmp.path}/${'a' * 32}.yfe');
    final w = await both[0].create(f, name: 'x', mime: 'x', size: 3);
    await w.add([1, 2, 3]);
    await w.close();
    expect((await both[0].readAll(f)), [1, 2, 3]);
    await v.lock();
    await expectLater(both[1].readAll(f), throwsA(isA<FileVaultException>()), reason: 'у каждого, кто успел взять объект');
    await expectLater(v.files(), throwsStateError);
  });

  test('files(), начатый до блокировки, после неё не выдаёт рабочий объект', () async {
    await unlocked();
    final pending = v.files();
    final lockF = v.lock();
    FileVault? got;
    try {
      got = await pending;
    } catch (_) {}
    await lockF;
    if (got != null) {
      expect(got.disposed, isTrue, reason: 'если и вернулся, то уже отозванным');
    }
    await expectLater(v.files(), throwsStateError);
  });

  test('блокировка во время открытия отменяет его: сеанс не публикуется, база закрывается (R12-7)', () async {
    await v.create('пароль-тест', null);
    final gate = Completer<void>();
    StorageService.debugOpen = (key) async {
      opens++;
      await gate.future;
    };
    final opening = v.unlock('пароль-тест');
    await Future<void>.delayed(const Duration(milliseconds: 600));   // ключ вычислен, база «открывается»
    expect(v.active, isTrue, reason: 'открытие в процессе считается активным: блокировка его увидит');
    await v.lock();
    gate.complete();
    await expectLater(opening, throwsStateError);
    expect(v.unlocked, isFalse);
    expect(v.active, isFalse);
    await expectLater(v.files(), throwsStateError);
    expect(closes, greaterThanOrEqualTo(2), reason: 'база, успевшая открыться позже блокировки, закрыта');
  });

  test('ошибка закрытия базы не оставляет приложение открытым (R12-8)', () async {
    await unlocked();
    expect(v.unlocked, isTrue);
    StorageService.debugClose = () async => throw StateError('не закрылась');
    await v.lock();
    expect(v.unlocked, isFalse);
    await expectLater(v.files(), throwsStateError);
    await expectLater(v.subKey('db'), throwsStateError);
  });

  test('кеш расшифрованных картинок: чтение, начатое до блокировки, после неё в кеш не возвращается (R12-10)', () async {
    await unlocked();
    final fv = await v.files();
    final f = File('${tmp.path}/${'b' * 32}.yfe');
    final w = await fv.create(f, name: 'p.png', mime: 'image/png', size: 4);
    await w.add([1, 2, 3, 4]);
    await w.close();
    final pending = VaultBytesCache.load(f.path);
    await v.lock();
    final b = await pending;
    expect(b, isNull, reason: 'результат устаревшего сеанса не отдаётся и не кладётся в кеш');
    await v.unlock('пароль-тест');   // хранилище уже создано, открываем заново
    final again = await VaultBytesCache.load(f.path);
    expect(again, isNotNull);
    await v.lock();
    expect(again!.every((x) => x == 0), isTrue, reason: 'сами буферы стёрты, а не только ссылки');
  });
}
