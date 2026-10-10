import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import '../crypto/file_crypto.dart';
import '../crypto/group_crypto.dart';
import 'group_api.dart';
import 'pool.dart';

/// Файлы в группах: зашифрованные куски лежат на узле, ключ файла — внутри зашифрованной записи журнала (узел его не видит).
class GroupFiles {
  final GroupApi api;
  GroupFiles(this.api);

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
    final size = await file.length();
    if (size == 0) throw const GroupApiError(400, 'Файл пустой');
    if (size > maxSize) throw const GroupApiError(413, 'В группе файлы до 200 МБ');
    final chunks = (size + chunkSize - 1) ~/ chunkSize;
    final key = FileCrypto.newKey();
    final started = await api.fileStart(gid, size, chunks);
    final raf = await file.open();
    final digest = _Sink();
    final hasher = crypto.sha256.startChunkedConversion(digest);
    try {
      // сначала хэш по порядку (заодно проверка, что файл не меняется), потом параллельная заливка
      for (var i = 0; i < chunks; i++) {
        await raf.setPosition(i * chunkSize);
        hasher.add(await raf.read(chunkSize));
      }
      hasher.close();
      var done = 0;
      await runPool(chunks, 4, (i) async {
        final plain = await _readAt(file, i * chunkSize);
        final wire = await FileCrypto.encryptChunk(key, started.fileId, i, chunks, plain);
        await api.filePut(gid, started.fileId, i, wire);
        onProgress?.call(++done, chunks);
      });
      await api.fileDone(gid, started.fileId);
    } catch (e) {
      try { await api.fileDelete(gid, started.fileId); } catch (_) {}
      rethrow;
    } finally {
      await raf.close();
    }
    return {'f': started.fileId, 'n': name, 'z': size, 'c': chunks, 'k': base64.encode(key), 'h': digest.hex};
  }

  static Future<List<int>> _readAt(File f, int pos) async {
    final r = await f.open();
    try {
      await r.setPosition(pos);
      return await r.read(chunkSize);
    } finally {
      await r.close();
    }
  }

  /// Скачивает, расшифровывает и проверяет файл; пишет в [out]. Бросает исключение при любом несовпадении (кусок, порядок, сумма).
  Future<void> download(String gid, Map<String, dynamic> d, File out, {void Function(int done, int total)? onProgress}) async {
    final fid = d['f'] as String, chunks = d['c'] as int, size = d['z'] as int;
    final key = base64.decode(d['k'] as String);
    if (chunks < 1 || size < 1 || size > maxSize || key.length != 32) throw const FormatException('bad file description');
    final sink = out.openWrite();
    final digest = _Sink();
    final hasher = crypto.sha256.startChunkedConversion(digest);
    var written = 0;
    try {
      var n = 0;
      await for (final plain in orderedPrefetch<Uint8List>(chunks, 4, (i) async => FileCrypto.decryptChunk(key, fid, i, chunks, await api.fileGet(gid, fid, i)))) {
        hasher.add(plain);
        sink.add(plain);
        written += plain.length;
        onProgress?.call(++n, chunks);
      }
      hasher.close();
      await sink.flush();
      await sink.close();
    } catch (e) {
      try { await sink.close(); } catch (_) {}
      try { await out.delete(); } catch (_) {}
      rethrow;
    }
    if (written != size || digest.hex != d['h']) {
      try { await out.delete(); } catch (_) {}
      throw const FormatException('файл повреждён или подменён');
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
