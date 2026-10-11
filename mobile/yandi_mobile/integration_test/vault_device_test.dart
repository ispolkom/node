import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:cryptography_flutter/cryptography_flutter.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';
import 'package:sqflite_sqlcipher/sqflite.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';
import 'package:yandi_mobile/models/message.dart';
import 'package:yandi_mobile/services/storage_service.dart';
import 'package:yandi_mobile/services/vault_backend.dart';
import 'package:yandi_mobile/services/vault_files.dart';

/// Проверки на настоящем Android (эмулятор или телефон): настоящий Keystore, SQLCipher, быстрое шифрование. Данные приложения
/// на устройстве, где это запускают, затираются, поэтому запускать только на эмуляторе или пустом тестовом телефоне.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  FlutterCryptography.enable();

  testWidgets('ключи на настоящем Keystore: создание, вход, слова, паузы', (t) async {
    final b = DeviceVaultBackend();
    await VaultKeys.destroy(b);
    final words = ['тайный', 'первый', 'слово', 'ёлочка'];
    var sw = Stopwatch()..start();
    final v = await VaultKeys.create(b, password: 'пароль-тест', phraseFields: words);
    // ignore: avoid_print
    print('TIMING create (оба замка, боевые параметры): ${sw.elapsedMilliseconds} мс');
    final key = await v.subKey('db');
    v.lock();
    expect(await StorageService.secureRead('vault_hw'), isNotNull, reason: 'секрет Keystore создан');
    sw = Stopwatch()..start();
    final again = await VaultKeys.unlock(b, 'пароль-тест');
    // ignore: avoid_print
    print('TIMING unlock паролем: ${sw.elapsedMilliseconds} мс');
    expect(await again.subKey('db'), key);
    again.lock();
    await expectLater(VaultKeys.unlock(b, 'не тот'), throwsA(isA<VaultWrongSecret>()));
    sw = Stopwatch()..start();
    final rec = await VaultKeys.recoverWithPhrase(b, ['ТАЙНЫЙ', 'первый', 'слово', 'елочка'], 'новый-пароль');
    // ignore: avoid_print
    print('TIMING восстановление по словам: ${sw.elapsedMilliseconds} мс');
    expect(await rec.subKey('db'), key);
    rec.lock();
    await VaultKeys.destroy(b);
    expect(await VaultKeys.exists(b), isFalse);
  });

  testWidgets('значения защищённого хранилища: обёртка и перенос на настоящем Keystore', (t) async {
    const raw = FlutterSecureStorage(aOptions: AndroidOptions(encryptedSharedPreferences: true));
    await raw.deleteAll();
    await raw.write(key: 'node_token_x', value: 'старый открытый токен');   // как было до обновления
    final key = List<int>.generate(32, (i) => i + 11);
    StorageService.setSecureKey(key);
    await StorageService.wrapExistingSecureValues();
    expect(await raw.read(key: 'node_token_x'), startsWith('e1:'));
    await StorageService.warmSecureCache();
    expect(await StorageService.secureRead('node_token_x'), 'старый открытый токен');
    await StorageService.secureWrite('id_ed25519_seed', 'seed-значение');
    expect(await raw.read(key: 'id_ed25519_seed'), isNot(contains('seed')));
    StorageService.setSecureKey(null);
    expect(await StorageService.secureRead('id_ed25519_seed'), isNull);
    await raw.deleteAll();
  });

  testWidgets('SQLCipher: зашифрованный файл, неверный ключ, перенос открытой базы', (t) async {
    final docs = await getApplicationDocumentsDirectory();
    final enc = File(p.join(docs.path, 'yandi_enc.db'));
    final plain = File(p.join(docs.path, 'yandi.db'));
    for (final f in [enc, plain]) {
      if (await f.exists()) await f.delete();
    }
    await StorageService.closeDb();
    final key = List<int>.generate(32, (i) => 200 - i);
    await StorageService.openEncrypted(key);
    await StorageService.saveMessage(ChatMessage(id: 'm1', peerId: 'peer', outgoing: true, text: 'очень секретный текст', timestamp: DateTime.now(), status: MessageStatus.delivered));
    await StorageService.closeDb();
    final bytes = enc.readAsBytesSync();
    expect(String.fromCharCodes(bytes.sublist(0, 16)), isNot(startsWith('SQLite format 3')));
    expect(String.fromCharCodes(bytes).contains('секрет'), isFalse);
    expect(String.fromCharCodes(bytes).contains('messages'), isFalse);
    // неверный ключ не открывает и не портит
    await expectLater(StorageService.openEncrypted(List<int>.filled(32, 5)), throwsA(anything));
    await StorageService.closeDb();
    await StorageService.openEncrypted(key);
    expect((await StorageService.db.query('messages')).single['text'], 'очень секретный текст');
    await StorageService.closeDb();
    await enc.delete();

    // старая открытая база переносится и стирается
    final old = await openDatabase(plain.path, version: 1, onCreate: (d, _) async {
      await d.execute('CREATE TABLE messages (id TEXT PRIMARY KEY, peer_id TEXT NOT NULL, outgoing INTEGER NOT NULL, text TEXT NOT NULL, timestamp INTEGER NOT NULL, status TEXT NOT NULL DEFAULT "pending")');
      await d.insert('messages', {'id': 'o1', 'peer_id': 'p', 'outgoing': 0, 'text': 'из старой базы', 'timestamp': 1, 'status': 'delivered'});
    });
    await old.close();
    await StorageService.openEncrypted(key);
    expect((await StorageService.db.query('messages')).single['text'], 'из старой базы');
    expect(plain.existsSync(), isFalse, reason: 'открытая база стёрта');
    await StorageService.closeDb();
    await enc.delete();
  });

  testWidgets('файлы: шифрование 6 МБ с быстрым AES, перенос старых открытых', (t) async {
    final tmp = await getTemporaryDirectory();
    final dir = Directory(p.join(tmp.path, 'itest_files'));
    if (await dir.exists()) await dir.delete(recursive: true);
    await dir.create(recursive: true);
    final key = Uint8List.fromList(List<int>.generate(32, (i) => i * 3));
    final vault = FileVault.withKey(key);
    final vf = VaultFiles(dir, vault: () async => vault, tempDir: Directory(p.join(dir.path, 'tmp')));
    final data = Uint8List.fromList(List.generate(6 * 1024 * 1024 + 123, (i) => Random(i).nextInt(256)));
    final src = File(p.join(tmp.path, 'itest_src.bin'))..writeAsBytesSync(data);
    var sw = Stopwatch()..start();
    final v = await vf.importFile(src.path, name: 'отчёт.pdf');
    // ignore: avoid_print
    print('TIMING шифрование 6 МБ: ${sw.elapsedMilliseconds} мс');
    sw = Stopwatch()..start();
    final back = await vault.readAll(File(v.path));
    // ignore: avoid_print
    print('TIMING расшифровка 6 МБ: ${sw.elapsedMilliseconds} мс');
    expect(back, data);
    expect((await vf.list()).single.name, 'отчёт.pdf');
    final src2 = await FileSource.open(v.path, vault: vault);
    expect(await src2.readAt(1024 * 1024 - 10, 20), data.sublist(1024 * 1024 - 10, 1024 * 1024 + 10));
    await src2.close();
    // старый открытый файл
    File(p.join(dir.path, 'aaaaaaaa_старый.txt')).writeAsStringSync('старый открытый текст');
    expect(await vf.migrateLegacy(), 1);
    expect((await vf.list()).map((f) => f.name).toSet(), {'отчёт.pdf', 'старый.txt'});
    expect(dir.listSync().whereType<File>().any((f) => f.path.endsWith('.txt')), isFalse);
    await dir.delete(recursive: true);
    await src.delete();
  });
}
