import 'dart:io';
import 'dart:typed_data';
import '../transport/api.dart';
import 'shell_repository.dart';

class FileRepository {
  final Api api;
  FileRepository(this.api);
  BinaryApi get binary {
    if (api is! BinaryApi) throw const ApiFailure('protocol');
    return api as BinaryApi;
  }

  static String mime(String name) =>
      switch (name.split('.').last.toLowerCase()) {
        'png' => 'image/png',
        'jpg' || 'jpeg' => 'image/jpeg',
        'gif' => 'image/gif',
        'webp' => 'image/webp',
        'mp3' => 'audio/mpeg',
        'ogg' => 'audio/ogg',
        'wav' => 'audio/wav',
        'm4a' || 'aac' => 'audio/mp4',
        'opus' => 'audio/ogg',
        'flac' => 'audio/flac',
        'mp4' || 'm4v' => 'video/mp4',
        'webm' => 'video/webm',
        'mov' => 'video/quicktime',
        'mkv' => 'video/x-matroska',
        '3gp' || '3gpp' => 'video/3gpp',
        'avi' => 'video/x-msvideo',
        'amr' => 'audio/amr',
        'mka' => 'audio/x-matroska',
        _ => 'application/octet-stream',
      };
  static bool raster(Uint8List b) =>
      b.length >= 12 &&
      (b[0] == 0x89 && b[1] == 0x50 && b[2] == 0x4e && b[3] == 0x47 ||
          b[0] == 0xff && b[1] == 0xd8 && b[2] == 0xff ||
          String.fromCharCodes(b.sublist(0, 4)) == 'GIF8' ||
          String.fromCharCodes(b.sublist(0, 4)) == 'RIFF' &&
              String.fromCharCodes(b.sublist(8, 12)) == 'WEBP');

  String path(Map<String, dynamic> attachment, String action) {
    final ref = attachment['file_ref'];
    if (ref is! Map || attachment['filename'] is! String)
      throw const ApiFailure('protocol');
    final result =
        '/api/files/$action/${ShellRepository(api).segment('${ref['file_id']}')}/${Uri.encodeComponent(attachment['filename'] as String)}';
    if (!validApiPath(result)) throw const ApiFailure('protocol');
    return result;
  }

  Future<Map<String, dynamic>> upload(
    String peer,
    File file, {
    required bool phone,
    required bool Function() isActive,
    required void Function(int, int) progress,
  }) async {
    final handle = await file.open();
    try {
      return await _push(
        peer,
        file.uri.pathSegments.last,
        await handle.length(),
        handle.read,
        phone: phone,
        isActive: isActive,
        progress: progress,
      );
    } finally {
      await handle.close();
    }
  }

  /// Отправляет уже полученные байты (пересылка файла другому собеседнику) тем же порядком кусков, что и файл с диска.
  Future<Map<String, dynamic>> uploadBytes(
    String peer,
    String name,
    Uint8List bytes, {
    required bool phone,
    required bool Function() isActive,
    void Function(int, int)? progress,
  }) {
    var at = 0;
    return _push(
      peer,
      name,
      bytes.length,
      (n) async {
        final end = (at + n).clamp(0, bytes.length);
        final part = Uint8List.fromList(bytes.sublist(at, end));
        at = end;
        return part;
      },
      phone: phone,
      isActive: isActive,
      progress: progress ?? (_, _) {},
    );
  }

  Future<Map<String, dynamic>> _push(
    String peer,
    String name,
    int size,
    Future<Uint8List> Function(int) read, {
    required bool phone,
    required bool Function() isActive,
    required void Function(int, int) progress,
  }) async {
    if (size == 0) throw const ApiFailure('empty_file');
    if (size > (phone ? 1024 : 200) * 1024 * 1024)
      throw const ApiFailure('file_limit');
    final type = mime(name);
    final chunkSize = phone ? 1024 * 1024 : 700;
    final total = (size / chunkSize).ceil();
    String? id;
    for (var i = 0; i < total; i++) {
      if (!isActive()) throw const ApiFailure('cancelled');
      final data = await read(chunkSize);
      if (data.length != (i == total - 1 ? size - i * chunkSize : chunkSize))
        throw const ApiFailure('protocol');
      Map<String, dynamic> result;
      try {
        result = await binary.multipart(
          '/api/files/send-chunk/${ShellRepository(api).segment(peer)}',
          {
            'filename': name,
            'mime_type': type,
            'file_size': '$size',
            'chunk_index': '$i',
            'total_chunks': '$total',
            if (id != null) 'file_id': id,
          },
          'file',
          name,
          data,
        );
      } finally {
        data.fillRange(0, data.length, 0);
      }
      if (!isActive()) throw const ApiFailure('cancelled');
      final returned = result['file_id'];
      if (returned is! String ||
          !RegExp(r'^[a-zA-Z0-9_-]{1,80}$').hasMatch(returned) ||
          id != null && id != returned ||
          result['complete'] != (i == total - 1) ||
          i < total - 1 && result['received'] != i + 1)
        throw const ApiFailure('protocol');
      id = returned;
      progress((i * chunkSize + data.length).clamp(0, size), size);
    }
    if (!isActive()) throw const ApiFailure('cancelled');
    final attachment = <String, dynamic>{
      'filename': name,
      'size': size,
      'mime_type': type,
      'file_ref': {'file_id': id, 'total_chunks': total},
    };
    await api.request(
      '/api/chat/send/${ShellRepository(api).segment(peer)}',
      body: {'text': '', 'attachment': attachment},
    );
    return attachment;
  }
}
