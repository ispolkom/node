import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:path_provider/path_provider.dart';
import '../crypto/file_crypto.dart';
import '../crypto/file_vault.dart';
import '../models/file_offer.dart';
import 'api_service.dart';
import 'pool.dart';

/// Размер куска в открытом виде. На проводе он на 28 байт больше (случайное число и метка шифрования).
const int chunkSize = 256 * 1024;
/// Файлы от 5 МБ идут быстрее: куски по 1 МБ и несколько запросов одновременно (узел принимает куски независимо). Раньше каждый кусок
/// ждал ответа предыдущего — скорость упиралась в задержку сети, а не в её ширину. Мелкие файлы — как прежде: по одному куску, 256 КБ.
const int bigFileSize = 5 * 1024 * 1024;
const int bigChunkSize = 1024 * 1024;
const int bigParallel = 4;

int chunkSizeFor(int fileSize) => fileSize >= bigFileSize ? bigChunkSize : chunkSize;
int parallelFor(int fileSize) => fileSize >= bigFileSize ? bigParallel : 1;
/// Самый большой файл (так же, как на узле).
const int maxFileSize = 1 << 30;
const int _tries = 4;

class TransferException implements Exception {
  final String message;
  TransferException(this.message);
  @override
  String toString() => message;
}

class _DigestSink implements Sink<crypto.Digest> {
  crypto.Digest? value;
  @override
  void add(crypto.Digest data) => value = data;
  @override
  void close() {}
}

class FileTransferService {
  final ApiService _api;
  /// Шифрование файлов хранилища ключом этого сеанса: принятые файлы пишутся сразу зашифрованными, зашифрованные читаются при отправке.
  final FileVault? vault;
  FileTransferService(this._api, {this.vault});

  /// Папка, куда складываются полученные файлы (внутри приложения).
  static Future<Directory> downloadsDir() async {
    final dir = await getApplicationDocumentsDirectory();
    final out = Directory('${dir.path}/yandi_downloads');
    if (!await out.exists()) await out.create(recursive: true);
    return out;
  }

  /// Имя без путей и служебных символов.
  static String safeName(String name) {
    var n = name.split(RegExp(r'[\\/]')).last.replaceAll(RegExp(r'[\u0000-\u001f<>:"|?*]'), '_').trim();
    while (n.startsWith('.')) { n = n.substring(1); }
    if (n.isEmpty) n = 'file';
    return n.length > 100 ? n.substring(n.length - 100) : n;
  }

  /// Имя файла на диске: безликий номер передачи (настоящее имя лежит внутри зашифрованного файла).
  static String localName(FileOffer o) => '${FileVault.safeId(o.tid)}${FileVault.extension}';

  static String guessMime(String name) {
    final ext = name.contains('.') ? name.split('.').last.toLowerCase() : '';
    const m = {
      'jpg': 'image/jpeg', 'jpeg': 'image/jpeg', 'png': 'image/png', 'gif': 'image/gif', 'webp': 'image/webp', 'bmp': 'image/bmp',
      'mp4': 'video/mp4', 'mkv': 'video/x-matroska', 'webm': 'video/webm', 'mov': 'video/quicktime',
      'mp3': 'audio/mpeg', 'ogg': 'audio/ogg', 'wav': 'audio/wav', 'm4a': 'audio/mp4', 'opus': 'audio/opus',
      'pdf': 'application/pdf', 'txt': 'text/plain', 'zip': 'application/zip', 'apk': 'application/vnd.android.package-archive',
      'doc': 'application/msword', 'docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      'xls': 'application/vnd.ms-excel', 'xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    };
    return m[ext] ?? 'application/octet-stream';
  }

  Future<T> _retry<T>(Future<T?> Function() op, String what) async {
    for (var i = 0; i < _tries; i++) {
      final r = await op();
      if (r != null) return r;
      await Future<void>.delayed(Duration(milliseconds: 400 * (1 << i)));
    }
    throw TransferException('$what: нет связи с узлом');
  }

  /// Шаг 1 отправки: занять передачу на узле и подготовить ключ. Ещё ничего не залито.
  Future<FileOffer> prepare({required String toPeerId, required String path, String? name}) async {
    final src = await FileSource.open(path, vault: vault);
    final size = src.length;
    await src.close();
    if (size == 0) throw TransferException('Файл пустой');
    if (size > maxFileSize) throw TransferException('Файл больше 1 ГБ');
    final fileName = safeName(name ?? path.split(Platform.pathSeparator).last);
    final chunks = max(1, (size / chunkSizeFor(size)).ceil());
    // узлу уходит безликая подпись: настоящее имя только внутри зашифрованного предложения
    final tid = await _api.startFileTransfer(toPeerId: toPeerId, fileName: 'file', fileSize: size, totalChunks: chunks);
    if (tid == null) throw TransferException('Узел не принял передачу (нет места, лимит или это не ваше устройство)');
    return FileOffer(
      tid: tid, name: fileName, size: size, chunks: chunks, mime: guessMime(fileName),
      key: base64.encode(FileCrypto.newKey()),
    );
  }

  /// Шаг 2: зашифровать и залить куски. Возвращает предложение с контрольной суммой.
  Future<FileOffer> upload(FileOffer offer, String path, {void Function(double)? onProgress}) async {
    final key = base64.decode(offer.key);
    final out = _DigestSink();
    final hash = crypto.sha256.startChunkedConversion(out);
    final src = await FileSource.open(path, vault: vault);
    final cs = chunkSizeFor(offer.size);
    var finished = 0;
    // чтение и контрольная сумма — строго по порядку (очередь), шифрование и заливка — параллельно (parallelFor)
    Future<void> readLock = Future.value();
    Future<Uint8List> readChunk(int i) {
      final result = Completer<Uint8List>();
      readLock = readLock.then((_) async {
        try {
          final want = min(cs, offer.size - i * cs);
          final Uint8List buf;
          try {
            buf = await src.readAt(i * cs, want);
          } on FileVaultException catch (e) {
            throw TransferException('Не удалось прочитать файл: $e');
          }
          if (buf.length != want) throw TransferException('Файл изменился во время отправки');
          hash.add(buf);
          result.complete(buf);
        } catch (e, st) {
          result.completeError(e, st);
        }
      });
      return result.future;
    }
    try {
      await runPool(offer.chunks, parallelFor(offer.size), (i) async {
        final buf = await readChunk(i);
        final wire = await FileCrypto.encryptChunk(key, offer.tid, i, offer.chunks, buf);
        await _retry<bool>(() async => await _api.uploadChunk(offer.tid, i, wire) ? true : null, 'кусок ${i + 1}');
        onProgress?.call(++finished / offer.chunks);
      });
    } finally {
      await src.close();
    }
    hash.close();
    await _retry<bool>(() async => await _api.completeTransfer(offer.tid) ? true : null, 'завершение');
    return offer.withSha(out.value.toString());
  }

  /// Скачать, расшифровать и сохранить. Проверяет контрольную сумму; при любой ошибке частичный файл удаляется.
  Future<String> download(FileOffer offer, {void Function(double)? onProgress}) async {
    final dir = await downloadsDir();
    final path = '${dir.path}/${localName(offer)}';
    final part = File('$path.part');
    final key = base64.decode(offer.key);
    final out = _DigestSink();
    final hash = crypto.sha256.startChunkedConversion(out);
    // с ключом хранилища файл пишется сразу зашифрованным (открытого содержимого на диске не бывает); без него — как раньше (тесты)
    final w = vault == null ? null : await vault!.create(part, name: safeName(offer.name), mime: offer.mime, size: offer.size);
    final sink = w == null ? part.openWrite() : null;
    var total = 0;
    try {
      var i = -1;
      // куски скачиваются с опережением (для больших файлов — по несколько сразу), а расшифровка, сумма и запись — строго по порядку
      await for (final wire in orderedPrefetch<List<int>>(
          offer.chunks, parallelFor(offer.size), (k) => _retry<List<int>>(() => _api.downloadChunk(offer.tid, k), 'кусок ${k + 1}'))) {
        i++;
        final Uint8List plain;
        try {
          plain = await FileCrypto.decryptChunk(key, offer.tid, i, offer.chunks, wire);
        } catch (_) {
          throw TransferException('Кусок ${i + 1} не прошёл проверку: файл повреждён или подменён');
        }
        total += plain.length;
        hash.add(plain);
        if (w != null) {
          try {
            await w.add(plain);
          } on FileVaultException {
            throw TransferException('Размер не совпал: получено больше, чем заявлено (${offer.size} байт)');
          }
        } else {
          sink!.add(plain);
        }
        onProgress?.call((i + 1) / offer.chunks);
      }
      hash.close();
      if (total != offer.size) throw TransferException('Размер не совпал: получено $total из ${offer.size} байт');
      if (offer.sha.isNotEmpty && out.value.toString() != offer.sha) {
        throw TransferException('Контрольная сумма не совпала: файл повреждён');
      }
      if (w != null) {
        await w.close();
      } else {
        await sink!.close();
      }
      final target = File(path);
      if (await target.exists()) await target.delete();
      await part.rename(path);
    } catch (_) {
      if (w != null) {
        await w.abort();
      } else {
        try { await sink!.close(); } catch (_) {}
      }
      if (await part.exists()) await part.delete();
      rethrow;
    }
    // файл у нас: на узле он больше не нужен
    unawaited(_api.deleteTransfer(offer.tid));
    return path;
  }
}
