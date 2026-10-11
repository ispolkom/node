import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

/// No server body, URL, password or headers in exception text or logs.
class ApiFailure implements Exception {
  final String kind;
  final int? status;
  const ApiFailure(this.kind, [this.status]);
  String get message => switch (kind) {
    'timeout' => 'Демон не ответил вовремя. Повторите вручную.',
    'large' => 'Ответ демона превышает допустимый размер.',
    'protocol' => 'Ответ демона не соответствует протоколу JSON.',
    'busy' => 'Подождите завершения текущих запросов.',
    'http' || 'business' => switch (status) {
      401 => 'Неверный секрет или доступ запрещён.',
      429 => 'Слишком много попыток. Подождите перед повтором.',
      501 =>
        'Действие не реализовано демоном. Управляйте службой YANDI через системные средства.',
      _ => 'Демон отклонил действие${status == null ? '' : ' (HTTP $status)'}.',
    },
    _ =>
      'Нет связи с демоном. Проверьте, что YANDI запущен под вашим пользователем и Unix-сокет доступен.',
  };
  @override
  String toString() => message;
}

String resolveSocket(List<String> args, Map<String, String> env) {
  String? override;
  for (var i = 0; i < args.length; i++) {
    if (args[i].startsWith('--socket=')) {
      override = args[i].substring(9);
    } else if (args[i] == '--socket') {
      if (++i == args.length)
        throw const FormatException('После --socket нужен путь');
      override = args[i];
    } else {
      throw const FormatException('Поддерживается только --socket');
    }
  }
  override ??= env['YANDI_UI_SOCKET'];
  if (override != null && override.isNotEmpty) {
    if (!override.startsWith('/') || override.contains('\u0000')) {
      throw const FormatException('Путь сокета должен быть абсолютным');
    }
    return override;
  }
  final xdg = env['XDG_DATA_HOME'];
  if (xdg != null && xdg.isNotEmpty) return '$xdg/yandi/run/yandi-ui.sock';
  final home = env['HOME'];
  if (home == null || home.isEmpty)
    throw const FormatException('Не задан HOME');
  return '$home/.local/share/yandi/run/yandi-ui.sock';
}

abstract class Api {
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
  });
  void cancel();
}

/// A short-lived, non-cookie client per RPC lets a deadline forcibly close IO.
/// There is no TCP fallback, proxy, redirect, persistence or automatic POST retry.
class UnixApi implements Api {
  final String socketPath;
  final Duration timeout;
  final int maxResponseBytes;
  final Set<HttpClient> _active = {};
  UnixApi(
    this.socketPath, {
    this.timeout = const Duration(seconds: 15),
    this.maxResponseBytes = 1024 * 1024,
  });

  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
  }) async {
    if (!RegExp(r'^/api/[a-z0-9_/]+$').hasMatch(path))
      throw const ApiFailure('protocol');
    if (_active.length >= 6) throw const ApiFailure('busy');
    final client = HttpClient()
      ..findProxy = ((_) => 'DIRECT')
      ..connectionTimeout = timeout
      ..maxConnectionsPerHost = 1;
    _active.add(client);
    client.connectionFactory = (uri, proxyHost, proxyPort) async {
      if (await FileSystemEntity.type(socketPath, followLinks: false) !=
          FileSystemEntityType.unixDomainSock) {
        throw const ApiFailure('disconnected');
      }
      return Socket.startConnect(
        InternetAddress(socketPath, type: InternetAddressType.unix),
        0,
      );
    };
    Uint8List? encoded;
    try {
      if (body != null)
        encoded = Uint8List.fromList(utf8.encode(jsonEncode(body)));
      return await (() async {
        final req = await client.openUrl(
          body == null ? 'GET' : 'POST',
          Uri(scheme: 'http', host: 'localhost', path: path),
        );
        req.followRedirects = false;
        req.headers.set(HttpHeaders.acceptHeader, 'application/json');
        final payload = encoded;
        if (payload != null) {
          req.headers.contentType = ContentType.json;
          req.contentLength = payload.length;
          req.add(payload);
          await req.flush();
          payload.fillRange(0, payload.length, 0);
          encoded = null;
        }
        final response = await req.close();
        if (response.isRedirect)
          throw ApiFailure('protocol', response.statusCode);
        if (response.contentLength > maxResponseBytes)
          throw const ApiFailure('large');
        if (response.headers.contentType?.mimeType != 'application/json')
          throw const ApiFailure('protocol');
        // Fixed buffer avoids accumulating an unbounded/chunked response.
        final buffer = Uint8List(maxResponseBytes);
        var length = 0;
        try {
          await for (final chunk in response) {
            if (length + chunk.length > maxResponseBytes)
              throw const ApiFailure('large');
            buffer.setRange(length, length + chunk.length, chunk);
            length += chunk.length;
          }
          final value = jsonDecode(
            utf8.decode(Uint8List.sublistView(buffer, 0, length)),
          );
          if (value is! Map<String, dynamic>)
            throw const ApiFailure('protocol');
          if (response.statusCode < 200 || response.statusCode >= 300)
            throw ApiFailure('http', response.statusCode);
          if (value['ok'] == false ||
              value['status'] == 'error' ||
              value['error'] != null &&
                  value['error'] != '' &&
                  path != '/api/vault/status') {
            throw ApiFailure('business', response.statusCode);
          }
          return value;
        } finally {
          buffer.fillRange(0, buffer.length, 0);
        }
      })().timeout(
        timeout,
        onTimeout: () {
          client.close(force: true);
          throw const ApiFailure('timeout');
        },
      );
    } on ApiFailure {
      rethrow;
    } on FormatException {
      throw const ApiFailure('protocol');
    } on TimeoutException {
      throw const ApiFailure('timeout');
    } catch (_) {
      throw const ApiFailure('disconnected');
    } finally {
      final remaining = encoded;
      remaining?.fillRange(0, remaining.length, 0);
      _active.remove(client);
      client.close(force: true);
    }
  }

  @override
  void cancel() {
    for (final client in _active) {
      client.close(force: true);
    }
    _active.clear();
  }
}

/// Mutable UTF-8 copy; Flutter editing and JSON still create immutable Strings.
class SecretBytes {
  final Uint8List bytes;
  SecretBytes(String value) : bytes = Uint8List.fromList(utf8.encode(value));
  String decode() => utf8.decode(bytes);
  void clear() => bytes.fillRange(0, bytes.length, 0);
}
