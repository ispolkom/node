import 'dart:typed_data';
import 'package:flutter/foundation.dart';
import '../crypto/file_vault.dart';
import '../crypto/vault_keys.dart';
import 'storage_service.dart';
import 'vault_backend.dart';
import 'vault_bytes_cache.dart';

/// Закрытая среда приложения: пароль (и, по желанию, секретные слова) → ключ данных → зашифрованная база.
/// Пока приложение заблокировано, ключа нет ни в памяти, ни на диске: база не открывается.
///
/// Переходы «открыть / заблокировать / стереть» отсчитываются поколением [_gen]: любое незаконченное открытие, увидев, что
/// поколение сменилось (пришла блокировка), само откатывает всё, что успело открыть, и не публикует сеанс.
class VaultService extends ChangeNotifier {
  VaultService._();
  static final VaultService instance = VaultService._();

  VaultBackend backend = DeviceVaultBackend();
  VaultKeys? _keys;
  FileVault? _files;
  Future<FileVault>? _filesPending;
  int _gen = 0;
  bool _opening = false;

  bool get unlocked => _keys?.unlocked ?? false;

  /// Открыт или открывается: блокировку нужно выполнять и в этом случае (иначе незаконченное открытие осталось бы живым).
  bool get active => unlocked || _opening;

  Future<bool> exists() => VaultKeys.exists(backend);

  /// Создать хранилище. Оно остаётся закрытым: пользователь сразу вводит пароль (это и есть проверка), и только потом открывается база.
  Future<void> create(String password, List<String>? phrase) async {
    final k = await VaultKeys.create(backend, password: password, phraseFields: phrase);
    k.lock();
  }

  Future<void> unlock(String password) async => _open(await VaultKeys.unlock(backend, password));

  Future<void> recoverWithPhrase(List<String> phrase, String newPassword) async =>
      _open(await VaultKeys.recoverWithPhrase(backend, phrase, newPassword));

  void _alive(int my) {
    if (my != _gen) throw StateError('открытие прервано блокировкой');
  }

  Future<void> _open(VaultKeys k) async {
    final my = ++_gen;
    _opening = true;
    Uint8List? sk, dk;
    try {
      sk = await k.subKey('secure');
      _alive(my);
      StorageService.setSecureKey(sk);
      await StorageService.wrapExistingSecureValues();
      _alive(my);
      dk = await k.subKey('db');
      _alive(my);
      await StorageService.openEncrypted(dk);
      _alive(my);
    } catch (e) {
      // откат: если блокировка уже прошла, она убрала ключи, но открывавшаяся база могла успеть открыться позже — закрываем её здесь
      if (my == _gen) {
        _opening = false;
        StorageService.setSecureKey(null);
      }
      try { await StorageService.closeDb(); } catch (_) {}
      k.lock();
      rethrow;
    } finally {
      sk?.fillRange(0, sk.length, 0);
      dk?.fillRange(0, dk.length, 0);
    }
    _opening = false;
    _keys = k;
    notifyListeners();
  }

  /// Ключ для конкретной цели (файлы и т. п.); пока заблокировано — ошибка.
  Future<Uint8List> subKey(String purpose) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    return k.subKey(purpose);
  }

  /// Шифрование файлов хранилища ключом этого сеанса; пока заблокировано — ошибка. Один объект на сеанс (параллельные вызовы
  /// получают один и тот же), ключ стирается при блокировке, уже открытые чтения и записи отзываются.
  Future<FileVault> files() {
    final f = _files;
    if (f != null && !f.disposed) return Future.value(f);
    return _filesPending ??= _makeFiles().whenComplete(() {
      _filesPending = null;
    });
  }

  Future<FileVault> _makeFiles() async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    final my = _gen;
    final key = await k.subKey('files');
    try {
      if (my != _gen || _keys == null) throw StateError('хранилище заблокировано');
      return _files = FileVault.withKey(key);
    } finally {
      key.fillRange(0, key.length, 0);   // у FileVault своя копия
    }
  }

  /// Блокировка. Сначала всё, что стирает ключи (оно не может сорваться), и только потом закрытие базы: ошибка закрытия не оставляет
  /// приложение открытым.
  Future<void> lock() async {
    _gen++;                      // отменяет любое незаконченное открытие и чтение ключа файлов
    _opening = false;
    final keys = _keys;
    _keys = null;
    _filesPending = null;
    _files?.dispose();
    _files = null;
    VaultBytesCache.clear();
    StorageService.setSecureKey(null);
    keys?.lock();
    notifyListeners();
    try {
      await StorageService.closeDb();
    } catch (_) {
      // ключей уже нет: база без ключа не читается; закрытие повторится при следующем входе/блокировке
    }
  }

  /// Проверить пароль, не открывая ничего (подтверждение опасных действий). Неверные попытки считаются и замедляются, как при входе.
  Future<void> verifyPassword(String password) async => (await VaultKeys.unlock(backend, password)).lock();

  Future<void> changePassword(String oldPassword, String newPassword) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    await k.changePassword(oldPassword, newPassword);
  }

  /// Удалить всё: ключи, база, файлы, секреты. После этого приложение как после установки.
  Future<String?> eraseEverything() async {
    _gen++;
    _opening = false;
    _filesPending = null;
    _files?.dispose();
    _files = null;
    VaultBytesCache.clear();
    _keys?.lock();
    _keys = null;
    await VaultKeys.destroy(backend);
    final left = await StorageService.eraseAllLocalData();
    notifyListeners();
    return left;
  }
}
