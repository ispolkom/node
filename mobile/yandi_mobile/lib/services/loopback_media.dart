import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

/// Serves one decrypted media file from memory on 127.0.0.1 under an unguessable
/// path, with Range support so the player can seek. Nothing is written to disk.
class LoopbackMedia {
  final Uint8List bytes;
  final String mime;
  HttpServer? _server;
  late final String token = _token();
  LoopbackMedia(this.bytes, this.mime);

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
      var start = 0, end = bytes.length - 1;
      final range = request.headers.value(HttpHeaders.rangeHeader);
      if (range != null) {
        final m = RegExp(r'^bytes=(\d*)-(\d*)$').firstMatch(range.trim());
        if (m == null || (m[1]!.isEmpty && m[2]!.isEmpty)) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          return;
        }
        if (m[1]!.isEmpty) {
          final n = int.parse(m[2]!);
          start = max(0, bytes.length - n);
        } else {
          start = int.parse(m[1]!);
          if (m[2]!.isNotEmpty) end = min(end, int.parse(m[2]!));
        }
        if (start > end || start >= bytes.length) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          response.headers.set(
            HttpHeaders.contentRangeHeader,
            'bytes */${bytes.length}',
          );
          return;
        }
        response.statusCode = HttpStatus.partialContent;
        response.headers.set(
          HttpHeaders.contentRangeHeader,
          'bytes $start-$end/${bytes.length}',
        );
      }
      response.contentLength = end - start + 1;
      if (request.method == 'GET') {
        response.add(Uint8List.sublistView(bytes, start, end + 1));
      }
    } finally {
      await response.close();
    }
  }

  Future<void> stop() async {
    await _server?.close(force: true);
    _server = null;
    bytes.fillRange(0, bytes.length, 0);
  }
}
