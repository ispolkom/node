import 'dart:typed_data';
import 'package:flutter/foundation.dart';
import '../crypto/file_vault.dart';
import '../crypto/vault_keys.dart';
import 'storage_service.dart';
import 'vault_bytes_cache.dart';
import 'vault_backend.dart';

/// Закрытая среда приложения: пароль (и, по желанию, секретные слова) → ключ данных → зашифрованная база.
/// Пока приложение заблокировано, ключа нет ни в памяти, ни на диске: база не открывается.
class VaultService extends ChangeNotifier {
  VaultService._();
  static final VaultService instance = VaultService._();

  final VaultBackend backend = DeviceVaultBackend();
  VaultKeys? _keys;
  FileVault? _files;

  bool get unlocked => _keys?.unlocked ?? false;

  Future<bool> exists() => VaultKeys.exists(backend);

  /// Создать хранилище. Оно остаётся закрытым: пользователь сразу вводит пароль (это и есть проверка), и только потом открывается база.
  Future<void> create(String password, List<String>? phrase) async {
    final k = await VaultKeys.create(backend, password: password, phraseFields: phrase);
    k.lock();
  }

  Future<void> unlock(String password) async => _open(await VaultKeys.unlock(backend, password));

  Future<void> recoverWithPhrase(List<String> phrase, String newPassword) async =>
      _open(await VaultKeys.recoverWithPhrase(backend, phrase, newPassword));

  Future<void> _open(VaultKeys k) async {
    try {
      StorageService.setSecureKey(await k.subKey('secure'));
      await StorageService.wrapExistingSecureValues();
      await StorageService.openEncrypted(await k.subKey('db'));
    } catch (e) {
      StorageService.setSecureKey(null);
      k.lock();
      rethrow;
    }
    _keys = k;
    notifyListeners();
  }

  /// Ключ для конкретной цели (файлы и т. п.); пока заблокировано — ошибка.
  Future<Uint8List> subKey(String purpose) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    return k.subKey(purpose);
  }

  /// Шифрование файлов хранилища ключом этого сеанса; пока заблокировано — ошибка. Ключ стирается при блокировке.
  Future<FileVault> files() async {
    var f = _files;
    if (f != null) return f;
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    return _files = FileVault.withKey(await k.subKey('files'));
  }

  /// Блокировка: база закрывается, ключ стирается из памяти.
  Future<void> lock() async {
    await StorageService.closeDb();
    StorageService.setSecureKey(null);
    _files?.dispose();
    _files = null;
    VaultBytesCache.clear();
    _keys?.lock();
    _keys = null;
    notifyListeners();
  }

  /// Проверить пароль, не открывая ничего (подтверждение опасных действий). Неверные попытки считаются и замедляются, как при входе.
  Future<void> verifyPassword(String password) async => (await VaultKeys.unlock(backend, password)).lock();

  Future<void> changePassword(String oldPassword, String newPassword) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    await k.changePassword(oldPassword, newPassword);
  }

  /// Удалить всё: ключи, база, файлы, секреты. После этого приложение как после установки.
  Future<void> eraseEverything() async {
    _files?.dispose();
    _files = null;
    _keys?.lock();
    _keys = null;
    await VaultKeys.destroy(backend);
    await StorageService.eraseAllLocalData();
    notifyListeners();
  }
}
