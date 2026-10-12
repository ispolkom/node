import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

/// Serves one decrypted media file from memory on 127.0.0.1 under an unguessable
/// path, with Range support so the player can seek. Nothing is written to disk.
class LoopbackMedia {
  /// Весь файл в памяти (маленькие файлы) — либо null, если данные читаются кусками по запросу.
  final Uint8List? bytes;
  final int length;
  final String mime;
  final Future<Uint8List> Function(int start, int len)? _read;
  HttpServer? _server;
  late final String token = _token();
  LoopbackMedia(Uint8List data, this.mime)
      : bytes = data,
        length = data.length,
        _read = null;

  /// Данные расшифровываются кусками по мере запросов плеера: в памяти одновременно только кусок, размер файла не ограничен.
  LoopbackMedia.reader(this.length, this.mime, Future<Uint8List> Function(int start, int len) read)
      : bytes = null,
        _read = read;

  /// Размер порции, которой отдаётся ответ.
  static const slice = 1 << 20;

  static String _token() {
    final r = Random.secure();
    return List.generate(
      24,
      (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0'),
    ).join();
  }

  Future<Uri> start() async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    _server = server;
    server.listen(_handle);
    return Uri.parse('http://127.0.0.1:${server.port}/$token');
  }

  Future<void> _handle(HttpRequest request) async {
    final response = request.response;
    try {
      if (request.uri.path != '/$token' ||
          (request.method != 'GET' && request.method != 'HEAD')) {
        response.statusCode = HttpStatus.notFound;
        return;
      }
      response.headers
        ..set(HttpHeaders.contentTypeHeader, mime)
        ..set(HttpHeaders.acceptRangesHeader, 'bytes')
        ..set(HttpHeaders.cacheControlHeader, 'no-store');
      var start = 0, end = length - 1;
      final range = request.headers.value(HttpHeaders.rangeHeader);
      if (range != null) {
        final m = RegExp(r'^bytes=(\d*)-(\d*)$').firstMatch(range.trim());
        if (m == null || (m[1]!.isEmpty && m[2]!.isEmpty)) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          return;
        }
        if (m[1]!.isEmpty) {
          final n = int.parse(m[2]!);
          start = max(0, length - n);
        } else {
          start = int.parse(m[1]!);
          if (m[2]!.isNotEmpty) end = min(end, int.parse(m[2]!));
        }
        if (start > end || start >= length) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          response.headers.set(
            HttpHeaders.contentRangeHeader,
            'bytes */${length}',
          );
          return;
        }
        response.statusCode = HttpStatus.partialContent;
        response.headers.set(
          HttpHeaders.contentRangeHeader,
          'bytes $start-$end/${length}',
        );
      }
      response.contentLength = end - start + 1;
      if (request.method == 'GET') {
        final data = bytes;
        if (data != null) {
          response.add(Uint8List.sublistView(data, start, end + 1));
        } else {
          var pos = start;
          while (pos <= end) {
            final n = min(slice, end - pos + 1);
            final part = await _read!(pos, n);
            if (part.isEmpty) break;
            response.add(part);
            await response.flush();
            pos += part.length;
          }
        }
      }
    } catch (_) {
      // плеер оборвал соединение (перемотка) — это нормально
    } finally {
      try {
        await response.close();
      } catch (_) {}
    }
  }

  Future<void> stop() async {
    await _server?.close(force: true);
    _server = null;
    bytes?.fillRange(0, length, 0);
  }
}
