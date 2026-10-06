import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:path_provider/path_provider.dart';
import 'api_service.dart';

const int _chunkSize = 256 * 1024; // 256 КБ

class UploadProgress {
  final int chunksTotal;
  final int chunksDone;
  final bool complete;
  final String? error;
  const UploadProgress({
    required this.chunksTotal,
    required this.chunksDone,
    this.complete = false,
    this.error,
  });
  double get fraction => chunksTotal == 0 ? 0 : chunksDone / chunksTotal;
}

class FileTransferService {
  final ApiService _api;
  FileTransferService(this._api);

  /// Загрузить файл на ноду кусками.
  ///
  /// [toPeerId]  — hex peer_id получателя
  /// [filePath]  — путь к локальному файлу
  /// [onProgress] — вызывается после каждого загруженного чанка
  ///
  /// Возвращает transfer_id при успехе, null при ошибке.
  Future<String?> uploadFile({
    required String toPeerId,
    required String filePath,
    void Function(UploadProgress)? onProgress,
  }) async {
    final file      = File(filePath);
    final fileSize  = await file.length();
    final fileName  = file.path.split(Platform.pathSeparator).last;
    final totalChunks = max(1, (fileSize / _chunkSize).ceil());

    final transferId = await _api.startFileTransfer(
      toPeerId:    toPeerId,
      fileName:    fileName,
      fileSize:    fileSize,
      totalChunks: totalChunks,
    );
    if (transferId == null) return null;

    onProgress?.call(UploadProgress(chunksTotal: totalChunks, chunksDone: 0));

    final raf = await file.open();
    try {
      for (int i = 0; i < totalChunks; i++) {
        final buf = Uint8List(min(_chunkSize, fileSize - i * _chunkSize));
        await raf.readInto(buf);
        final ok = await _api.uploadChunk(transferId, i, buf);
        if (!ok) {
          onProgress?.call(UploadProgress(
            chunksTotal: totalChunks, chunksDone: i, error: 'chunk $i failed'));
          return null;
        }
        onProgress?.call(UploadProgress(
          chunksTotal: totalChunks, chunksDone: i + 1));
      }
    } finally {
      await raf.close();
    }

    final done = await _api.completeTransfer(transferId);
    if (!done) return null;

    onProgress?.call(UploadProgress(
      chunksTotal: totalChunks, chunksDone: totalChunks, complete: true));
    return transferId;
  }

  /// Скачать файл с ноды и сохранить в папку загрузок.
  ///
  /// Возвращает путь к сохранённому файлу при успехе.
  Future<String?> downloadFile({
    required String transferId,
    required String fileName,
    required int totalChunks,
    void Function(UploadProgress)? onProgress,
  }) async {
    final dir = await getApplicationDocumentsDirectory();
    final outDir  = Directory('${dir.path}/yandi_downloads');
    await outDir.create(recursive: true);
    final outPath = '${outDir.path}/$fileName';
    final outFile = File(outPath);
    final sink    = outFile.openWrite();

    onProgress?.call(UploadProgress(chunksTotal: totalChunks, chunksDone: 0));

    try {
      for (int i = 0; i < totalChunks; i++) {
        final chunk = await _api.downloadChunk(transferId, i);
        if (chunk == null) {
          onProgress?.call(UploadProgress(
            chunksTotal: totalChunks, chunksDone: i, error: 'chunk $i failed'));
          return null;
        }
        sink.add(chunk);
        onProgress?.call(UploadProgress(
          chunksTotal: totalChunks, chunksDone: i + 1));
      }
    } finally {
      await sink.close();
    }

    onProgress?.call(UploadProgress(
      chunksTotal: totalChunks, chunksDone: totalChunks, complete: true));
    return outPath;
  }
}
