import 'dart:typed_data';
import 'package:flutter/foundation.dart';
import '../crypto/vault_keys.dart';
import 'storage_service.dart';
import 'vault_backend.dart';

/// Закрытая среда приложения: пароль (и, по желанию, секретные слова) → ключ данных → зашифрованная база.
/// Пока приложение заблокировано, ключа нет ни в памяти, ни на диске: база не открывается.
class VaultService extends ChangeNotifier {
  VaultService._();
  static final VaultService instance = VaultService._();

  final VaultBackend backend = DeviceVaultBackend();
  VaultKeys? _keys;

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
      await StorageService.openEncrypted(await k.subKey('db'));
    } catch (e) {
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

  /// Блокировка: база закрывается, ключ стирается из памяти.
  Future<void> lock() async {
    await StorageService.closeDb();
    StorageService.dropSecureCache();
    _keys?.lock();
    _keys = null;
    notifyListeners();
  }

  Future<void> changePassword(String oldPassword, String newPassword) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    await k.changePassword(oldPassword, newPassword);
  }

  /// Удалить всё: ключи, база, файлы, секреты. После этого приложение как после установки.
  Future<void> eraseEverything() async {
    _keys?.lock();
    _keys = null;
    await VaultKeys.destroy(backend);
    await StorageService.eraseAllLocalData();
    notifyListeners();
  }
}
