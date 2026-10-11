import 'dart:io';
import 'package:path_provider/path_provider.dart';
import '../crypto/vault_keys.dart';
import 'storage_service.dart';

/// Боевое хранилище ключей: файл в закрытой папке приложения (запись через временный файл и переименование, чтобы обрыв не оставил
/// половину) и защищённое хранилище Android (Keystore) для секрета `hw` и счётчиков попыток.
class DeviceVaultBackend implements VaultBackend {
  File? _file;

  Future<File> _f() async {
    final f = _file;
    if (f != null) return f;
    final dir = await getApplicationSupportDirectory();
    return _file = File('${dir.path}/vault_keys.json');
  }

  @override
  Future<String?> readBlob() async {
    final f = await _f();
    if (!await f.exists()) return null;
    return f.readAsString();
  }

  @override
  Future<void> writeBlob(String text) async {
    final f = await _f();
    final tmp = File('${f.path}.tmp');
    await tmp.writeAsString(text, flush: true);
    await tmp.rename(f.path);
  }

  @override
  Future<String?> secureRead(String key) => StorageService.secureRead(key);
  @override
  Future<void> secureWrite(String key, String value) => StorageService.secureWrite(key, value);
  @override
  Future<void> secureDelete(String key) => StorageService.secureDelete(key);
  @override
  int nowMs() => DateTime.now().millisecondsSinceEpoch;
}
