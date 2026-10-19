import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import '../crypto/file_crypto.dart';
import '../crypto/file_vault.dart';
import '../crypto/group_crypto.dart';
import 'file_transfer_service.dart' show FileTransferService;
import 'group_api.dart';
import 'pool.dart';
import '../l10n_core.dart';

/// Файлы в группах: зашифрованные куски лежат на узле, ключ файла — внутри зашифрованной записи журнала (узел его не видит).
class GroupFiles {
  final GroupApi api;
  /// Шифрование файлов хранилища ключом этого сеанса: скачанные файлы пишутся зашифрованными, зашифрованные читаются при отправке.
  final FileVault? vault;
  GroupFiles(this.api, {this.vault});

  /// Самый большой файл группы (узел отказывает сверх этого).
  static const maxSize = 200 << 20;
  static const chunkSize = 256 * 1024;

  /// Вид записи по имени файла: так работают разделы «Картинки / Аудио / Видео / Файлы».
  static int kindForName(String name) {
    final ext = name.contains('.') ? name.split('.').last.toLowerCase() : '';
    if (const {'jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'heic'}.contains(ext)) return GroupKind.image;
    if (const {'mp3', 'ogg', 'oga', 'opus', 'wav', 'flac', 'm4a', 'aac'}.contains(ext)) return GroupKind.audio;
    if (const {'mp4', 'mkv', 'webm', 'mov', 'avi', '3gp'}.contains(ext)) return GroupKind.video;
    return GroupKind.file;
  }

  /// Заливает файл и возвращает описание для записи журнала: {f: номер на узле, n: имя, z: размер, c: кусков, k: ключ, h: sha256}.
  Future<Map<String, dynamic>> upload(String gid, File file, String name, {void Function(int done, int total)? onProgress}) async {
    final src = await FileSource.open(file.path, vault: vault);
    final size = src.length;
    if (size == 0) {
      await src.close();
      throw GroupApiError(400, tr('Файл пустой'));
    }
    if (size > maxSize) {
      await src.close();
      throw GroupApiError(413, tr('В группе файлы до 200 МБ'));
    }
    final chunks = (size + chunkSize - 1) ~/ chunkSize;
    final key = FileCrypto.newKey();
    final started = await api.fileStart(gid, size, chunks);
    final digest = _Sink();
    final hasher = crypto.sha256.startChunkedConversion(digest);
    try {
      // сначала хэш по порядку (заодно проверка, что файл не меняется), потом параллельная заливка
      for (var i = 0; i < chunks; i++) {
        hasher.add(await src.readAt(i * chunkSize, chunkSize));
      }
      hasher.close();
      var done = 0;
      await runPool(chunks, 4, (i) async {
        final plain = await src.readAt(i * chunkSize, chunkSize);
        final wire = await FileCrypto.encryptChunk(key, started.fileId, i, chunks, plain);
        await api.filePut(gid, started.fileId, i, wire);
        onProgress?.call(++done, chunks);
      });
      await api.fileDone(gid, started.fileId);
    } catch (e) {
      try { await api.fileDelete(gid, started.fileId); } catch (_) {}
      rethrow;
    } finally {
      await src.close();
    }
    return {'f': started.fileId, 'n': name, 'z': size, 'c': chunks, 'k': base64.encode(key), 'h': digest.hex};
  }

  /// Скачивает, расшифровывает и проверяет файл; пишет в [out]. Бросает исключение при любом несовпадении (кусок, порядок, сумма).
  Future<void> download(String gid, Map<String, dynamic> d, File out, {void Function(int done, int total)? onProgress}) async {
    final fid = d['f'] as String, chunks = d['c'] as int, size = d['z'] as int;
    final key = base64.decode(d['k'] as String);
    if (chunks < 1 || size < 1 || size > maxSize || key.length != 32) throw const FormatException('bad file description');
    // число кусков однозначно задаётся размером: описание с огромным `c` при малом размере не вызывает миллионы запросов
    if (chunks != (size + chunkSize - 1) ~/ chunkSize) throw const FormatException('bad file description');
    // с ключом хранилища файл пишется сразу зашифрованным; без него — как раньше (тесты)
    final name = d['n'] is String ? d['n'] as String : 'file';
    final w = vault == null ? null : await vault!.create(out, name: name, mime: FileTransferService.guessMime(name), size: size);
    final sink = w == null ? out.openWrite() : null;
    final digest = _Sink();
    final hasher = crypto.sha256.startChunkedConversion(digest);
    var written = 0;
    try {
      var n = 0;
      var idx = 0;   // номер куска: onProgress может быть не задан, поэтому по n считать нельзя
      await for (final plain in orderedPrefetch<Uint8List>(chunks, 4, (i) async => FileCrypto.decryptChunk(key, fid, i, chunks, await api.fileGet(gid, fid, i)))) {
        // точная длина каждого куска: все полные, кроме последнего, и пустых не бывает
        final expectLen = idx < chunks - 1 ? chunkSize : size - (chunks - 1) * chunkSize;
        if (plain.length != expectLen) throw FormatException(tr('файл повреждён или подменён'));
        hasher.add(plain);
        written += plain.length;
        if (written > size) throw FormatException(tr('файл повреждён или подменён'));
        if (w != null) {
          await w.add(plain);
        } else {
          sink!.add(plain);
        }
        idx++;
        onProgress?.call(++n, chunks);
      }
      hasher.close();
      if (written != size || digest.hex != d['h']) throw FormatException(tr('файл повреждён или подменён'));
      if (w != null) {
        await w.close();
      } else {
        await sink!.flush();
        await sink.close();
      }
    } catch (e) {
      if (w != null) {
        await w.abort();
      } else {
        try { await sink!.close(); } catch (_) {}
        try { await out.delete(); } catch (_) {}
      }
      if (e is FileVaultException) throw FormatException(tr('файл повреждён или подменён'));
      rethrow;
    }
  }
}

class _Sink implements Sink<crypto.Digest> {
  String hex = '';
  @override
  void add(crypto.Digest d) => hex = d.toString();
  @override
  void close() {}
}
