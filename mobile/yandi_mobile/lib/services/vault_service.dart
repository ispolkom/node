import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/foundation.dart';
import '../crypto/file_vault.dart';
import '../crypto/vault_keys.dart';
import 'storage_service.dart';
import 'vault_backend.dart';
import 'vault_bytes_cache.dart';
import 'video_frames.dart';

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
  Future<void> _dbTail = Future.value();
  bool _dbBusy = false;

  /// Открытие, откат и закрытие общей БД не пересекаются между поколениями.
  Future<T> _transition<T>(Future<T> Function() operation) {
    final result = _dbTail.then((_) async {
      _dbBusy = true;
      try { return await operation(); } finally { _dbBusy = false; }
    });
    _dbTail = result.then((_) {}, onError: (Object _, StackTrace __) {});
    return result;
  }

  bool get unlocked => _keys?.unlocked ?? false;

  /// Открыт или открывается: блокировку нужно выполнять и в этом случае (иначе незаконченное открытие осталось бы живым).
  bool get active => unlocked || _opening;

  Future<bool> exists() => VaultKeys.exists(backend);

  /// Создать хранилище. Оно остаётся закрытым: пользователь сразу вводит пароль (это и есть проверка), и только потом открывается база.
  Future<void> create(String password, List<String>? phrase) async {
    final k = await VaultKeys.create(backend, password: password, phraseFields: phrase);
    k.lock();
  }

  Future<void> unlock(String password) => _enter((_) => VaultKeys.unlock(backend, password));

  Future<void> recoverWithPhrase(List<String> phrase, String newPassword) =>
      _enter((my) => VaultKeys.recoverWithPhrase(backend, phrase, newPassword, beforeCommit: () => _alive(my)));

  Future<void> _enter(Future<VaultKeys> Function(int) derive) async {
    final my = _beginOpen();
    VaultKeys? keys;
    try {
      keys = await derive(my);
      _alive(my);
      await _open(keys, generation: my);
    } catch (_) {
      keys?.lock();
      if (my == _gen) _opening = false;
      rethrow;
    }
  }

  int _beginOpen() {
    _revoke();
    _opening = true;   // ДО первого ожидания Keystore/KDF: наблюдатель уже видит активную операцию
    return _gen;
  }

  void _alive(int my) {
    if (my != _gen) throw StateError('открытие прервано блокировкой');
  }

  Future<void> _open(VaultKeys k, {int? generation}) async {
    final my = generation ?? _beginOpen();
    await _transition(() async {
    Uint8List? sk, dk;
    try {
      _alive(my);
      await StorageService.closeDb();
      _alive(my);
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
      // Откат выполняется под тем же замком, что открытие: БД нового сеанса ещё не могла открыться.
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
    });
  }

  /// Ключ для конкретной цели (файлы и т. п.); пока заблокировано — ошибка.
  Future<Uint8List> subKey(String purpose) async {
    final k = _keys;
    if (k == null) throw StateError('хранилище заблокировано');
    final my = _gen;
    final key = await k.subKey(purpose);
    if (my != _gen || !identical(k, _keys)) {
      key.fillRange(0, key.length, 0);
      throw StateError('хранилище заблокировано');
    }
    return key;
  }

  /// Шифрование файлов хранилища ключом этого сеанса; пока заблокировано — ошибка. Один объект на сеанс (параллельные вызовы
  /// получают один и тот же), ключ стирается при блокировке, уже открытые чтения и записи отзываются.
  Future<FileVault> files() {
    final f = _files;
    if (f != null && !f.disposed) return Future.value(f);
    final pending = _filesPending;
    if (pending != null) return pending;
    late final Future<FileVault> next;
    next = _makeFiles().whenComplete(() {
      if (identical(_filesPending, next)) _filesPending = null;
    });
    return _filesPending = next;
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
  void _revoke() {
    _gen++;                      // отменяет любое незаконченное открытие и чтение ключа файлов
    _opening = false;
    final keys = _keys;
    _keys = null;
    _filesPending = null;
    _files?.dispose();
    _files = null;
    VaultBytesCache.clear();
    VideoFrames.clear();         // готовые кадры превью видео — тоже расшифрованное содержимое
    StorageService.setSecureKey(null);
    keys?.lock();
    notifyListeners();
  }

  Future<void> lock() async {
    _revoke();
    final busy = _dbBusy;
    final closing = _transition(() async {
      try { await StorageService.closeDb(); } catch (_) {}
    });
    // Ключи отозваны сразу; долгий перенос не задерживает показ закрытого экрана.
    // Закрытие в очереди обязательно завершится раньше следующего открытия.
    if (!busy) await closing;
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
    _revoke();
    return _transition(() async {
    await VaultKeys.destroy(backend);
    final left = await StorageService.eraseAllLocalData();
    notifyListeners();
    return left;
    });
  }
}
