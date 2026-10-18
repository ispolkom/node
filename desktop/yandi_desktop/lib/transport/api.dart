import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

/// No server body, URL, password or headers in exception text or logs.
class ApiFailure implements Exception {
  final String kind;
  final int? status;
  /// Только для восстановления из копии: что не очищено, пропущено или не подтверждено (пути и фиксированные сообщения узла); ограниченно по числу и длине.
  final List<String> warnings;
  /// Только для восстановления из копии: остались ли на диске изменённые файлы.
  final bool filesWritten;
  const ApiFailure(this.kind, [this.status, this.warnings = const [], this.filesWritten = false]);
  String get message => switch (kind) {
    'sealed_required' =>
      'Создайте и откройте закрытую среду перед изменением пула прокси.',
    'timeout' => 'Демон не ответил вовремя. Повторите вручную.',
    'large' => 'Ответ демона превышает допустимый размер.',
    'protocol' => 'Ответ демона не соответствует протоколу JSON.',
    'file_limit' => 'Демон: превышен допустимый размер файла.',
    'file_busy' => 'Демон: слишком много незавершённых загрузок.',
    'file_invalid' => 'Демон: неверный идентификатор файла.',
    'avatar_limit' => 'Аватар должен быть меньше 2 МБ.',
    'avatar_invalid' => 'Аватар должен быть картинкой PNG, JPEG или GIF.',
    'empty_file' => 'Пустые файлы не поддерживаются демоном.',
    'exists' => 'Файл уже существует. Выберите новое имя.',
    'cancelled' => 'Операция прервана.',
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

/// Отказ восстановления из копии: из тела ответа берутся только предупреждения (не больше 20 строк по 500 знаков) и признак «файлы записаны»;
/// остальное тело в исключение не попадает (P19-3, P20-6).
ApiFailure restoreFailureFrom(int status, Map<String, dynamic> value) {
  final w = value['warnings'];
  final list = <String>[
    if (w is List)
      for (final x in w.take(20))
        if (x is String) x.length > 500 ? x.substring(0, 500) : x,
  ];
  return ApiFailure('http', status, list, value['files_written'] == true);
}

String fileErrorKind(dynamic message) => switch (message) {
  'File is larger than the allowed size' => 'file_limit',
  'Too many unfinished uploads' => 'file_busy',
  'Invalid file_id' => 'file_invalid',
  _ => 'business',
};

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

/// Токен сеанса администратора (его выдаёт узел после админ-пароля). Живёт только в памяти окна: другой клиент того же пользователя его не знает.
class AdminToken {
  static String? value;
}

abstract class Api {
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  });
  void cancel();
}

/// Binary data never enters cookies, caches or a TCP connection.
abstract class BinaryApi {
  Future<Map<String, dynamic>> multipart(
    String path,
    Map<String, String> fields,
    String field,
    String filename,
    Uint8List bytes,
  );
  Future<Uint8List> bytes(String path, {int limit = 8 * 1024 * 1024});
  Future<void> download(
    String path,
    File destination, {
    required int limit,
    required bool Function() isActive,
  });
}

abstract class GroupFileApi {
  Future<Map<String, dynamic>> uploadGroup(
    String path,
    File file, {
    required bool Function() isActive,
    required void Function(int, int) progress,
  });
}

bool validApiPath(String path) {
  final upload = RegExp(
    r'^/api/groups/[a-f0-9]{32}/upload\?name=([^/?#]+)$',
  ).firstMatch(path);
  if (upload != null) {
    try {
      final name = Uri.decodeComponent(upload[1]!);
      return name.isNotEmpty &&
          name.length <= 255 &&
          !name.contains(RegExp(r'[/\\\x00-\x1f\x7f]')) &&
          Uri.encodeComponent(name) == upload[1];
    } catch (_) {
      return false;
    }
  }
  if (RegExp(r'^/api/[a-zA-Z0-9_/-]+$').hasMatch(path)) return true;
  if (RegExp(r'^/api/mobile/call/events\?after=[0-9]{1,20}$').hasMatch(path))
    return true;
  // код сопряжения для постороннего пользователя узла (роль — из закрытого списка)
  if (RegExp(r'^/api/mobile/pairing\?role=(owner|user)$').hasMatch(path)) {
    return true;
  }
  final match = RegExp(
    r'^/api/files/(content|status)/[a-zA-Z0-9_-]{1,128}/([^/?#]+)$',
  ).firstMatch(path);
  if (match == null) return false;
  try {
    final name = Uri.decodeComponent(match.group(2)!);
    return name.isNotEmpty &&
        name != '.' &&
        name != '..' &&
        !name.contains(RegExp(r'[/\\\x00-\x1f\x7f]')) &&
        name.length <= 255 &&
        Uri.encodeComponent(name) == match.group(2);
  } catch (_) {
    return false;
  }
}

/// A short-lived, non-cookie client per RPC lets a deadline forcibly close IO.
/// There is no TCP fallback, proxy, redirect, persistence or automatic POST retry.
class UnixApi implements Api, BinaryApi, GroupFileApi {
  final String socketPath;
  final Duration timeout;
  final int maxResponseBytes;
  final Set<HttpClient> _active = {};
  final Set<WebSocket> _sockets = {};
  UnixApi(
    this.socketPath, {
    this.timeout = const Duration(seconds: 15),
    this.maxResponseBytes = 1024 * 1024,
  });

  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    return await _exchange(path, body: body, method: method)
        as Map<String, dynamic>;
  }

  Future<Object> _exchange(
    String path, {
    Map<String, dynamic>? body,
    String? method,
    Uint8List? multipartBody,
    File? rawFile,
    void Function(int, int)? progress,
    String? boundary,
    int? binaryLimit,
    File? destination,
    bool Function()? isActive,
  }) async {
    if (!validApiPath(path) ||
        (method != null &&
            !const [
              'GET',
              'POST',
              'PUT',
              'PATCH',
              'DELETE',
            ].contains(method))) {
      throw const ApiFailure('protocol');
    }
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
      encoded = multipartBody;
      if (body != null)
        encoded = Uint8List.fromList(utf8.encode(jsonEncode(body)));
      return await (() async {
        final req = await client.openUrl(
          method ?? (body == null && rawFile == null ? 'GET' : 'POST'),
          Uri.parse('http://localhost$path'),
        );
        req.followRedirects = false;
        req.headers.set(HttpHeaders.acceptHeader, 'application/json');
        final adminToken = AdminToken.value;
        if (adminToken != null) req.headers.set('x-yandi-admin', adminToken);
        final payload = encoded;
        if (payload != null) {
          req.headers.contentType = boundary == null
              ? ContentType.json
              : ContentType(
                  'multipart',
                  'form-data',
                  parameters: {'boundary': boundary},
                );
          req.contentLength = payload.length;
          req.add(payload);
          await req.flush();
          payload.fillRange(0, payload.length, 0);
          encoded = null;
        }
        if (rawFile != null) {
          final handle = await rawFile.open();
          try {
            final size = await handle.length();
            if (size <= 0) throw const ApiFailure('empty_file');
            if (size > 200 * 1024 * 1024) throw const ApiFailure('file_limit');
            req.headers.contentType = ContentType.binary;
            req.contentLength = size;
            var sent = 0;
            while (sent < size) {
              if (isActive?.call() != true) throw const ApiFailure('cancelled');
              final chunk = await handle.read((size - sent).clamp(0, 65536));
              if (chunk.isEmpty) throw const ApiFailure('protocol');
              try {
                req.add(chunk);
                await req.flush();
                sent += chunk.length;
                progress?.call(sent, size);
              } finally {
                chunk.fillRange(0, chunk.length, 0);
              }
            }
          } finally {
            await handle.close();
          }
        }
        final response = await req.close();
        if (response.isRedirect)
          throw ApiFailure('protocol', response.statusCode);
        if (binaryLimit != null) {
          if (response.statusCode != 200)
            throw ApiFailure('http', response.statusCode);
          if (response.contentLength > binaryLimit)
            throw const ApiFailure('large');
          final builder = destination == null
              ? BytesBuilder(copy: false)
              : null;
          RandomAccessFile? output;
          var length = 0;
          var created = false, completed = false;
          try {
            if (destination != null) {
              if (isActive?.call() != true) throw const ApiFailure('cancelled');
              // Exclusive creation: never truncate existing files or follow symlinks.
              await destination.create(exclusive: true);
              created = true;
              output = await destination.open(mode: FileMode.writeOnly);
            }
            await for (final chunk in response) {
              if (isActive != null && !isActive())
                throw const ApiFailure('cancelled');
              length += chunk.length;
              if (length > binaryLimit) throw const ApiFailure('large');
              if (output != null) {
                await output.writeFrom(chunk);
              } else {
                builder!.add(chunk);
              }
            }
            if (response.contentLength >= 0 && length != response.contentLength)
              throw const ApiFailure('protocol');
            completed = true;
            return builder?.takeBytes() ?? <String, dynamic>{'ok': true};
          } finally {
            await output?.close();
            if (created && !completed) await destination!.delete();
          }
        }
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
          var value = jsonDecode(
            utf8.decode(Uint8List.sublistView(buffer, 0, length)),
          );
          if (value == null &&
              const [
                '/api/media/incoming-call',
                '/api/media/video/incoming-call',
              ].contains(path))
            value = <String, dynamic>{'incoming': null};
          if (path == '/api/groups/devices' && value is List)
            value = <String, dynamic>{'devices': value};
          if (value is! Map<String, dynamic>)
            throw const ApiFailure('protocol');
          if (response.statusCode < 200 || response.statusCode >= 300) {
            // единственное исключение из правила «тело ответа в ошибку не попадает»: ограниченный контракт восстановления из копии,
            // иначе владелец не узнал бы путь неочищенного остатка (P19-3); тип, число и длина проверяются
            if (path == '/api/auth/restore') throw restoreFailureFrom(response.statusCode, value);
            throw ApiFailure('http', response.statusCode);
          }
          if (value['ok'] == false ||
              value['status'] == 'error' ||
              value['error'] != null &&
                  value['error'] != '' &&
                  path != '/api/vault/status') {
            throw ApiFailure(
              fileErrorKind(value['message']),
              response.statusCode,
            );
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
  Future<Map<String, dynamic>> uploadGroup(
    String path,
    File file, {
    required bool Function() isActive,
    required void Function(int, int) progress,
  }) async =>
      await _exchange(
            path,
            rawFile: file,
            isActive: isActive,
            progress: progress,
          )
          as Map<String, dynamic>;

  @override
  Future<Map<String, dynamic>> multipart(
    String path,
    Map<String, String> fields,
    String field,
    String filename,
    Uint8List bytes,
  ) async {
    if (!(RegExp(
              r'^/api/files/send-chunk/[a-zA-Z0-9_-]{1,128}$',
            ).hasMatch(path) ||
            path == '/api/profile/avatar') ||
        !RegExp(r'^[a-z_]+$').hasMatch(field) ||
        fields.keys.any((key) => !RegExp(r'^[a-z_]+$').hasMatch(key))) {
      throw const ApiFailure('protocol');
    }
    final boundary = 'yandi_${DateTime.now().microsecondsSinceEpoch}';
    final payload = BytesBuilder();
    void add(String value) => payload.add(utf8.encode(value));
    for (final entry in fields.entries) {
      add(
        '--$boundary\r\nContent-Disposition: form-data; name="${entry.key}"\r\n\r\n${entry.value}\r\n',
      );
    }
    // The filename text field carries the real name. Never interpolate a selected name into headers.
    add(
      '--$boundary\r\nContent-Disposition: form-data; name="$field"; filename="upload"\r\nContent-Type: application/octet-stream\r\n\r\n',
    );
    payload.add(bytes);
    add('\r\n--$boundary--\r\n');
    final encoded = payload.takeBytes();
    try {
      return await _exchange(
            path,
            method: 'POST',
            multipartBody: encoded,
            boundary: boundary,
          )
          as Map<String, dynamic>;
    } finally {
      encoded.fillRange(0, encoded.length, 0);
    }
  }

  @override
  Future<Uint8List> bytes(String path, {int limit = 8 * 1024 * 1024}) async =>
      await _exchange(path, binaryLimit: limit) as Uint8List;

  @override
  Future<void> download(
    String path,
    File destination, {
    required int limit,
    required bool Function() isActive,
  }) async {
    // Only explicit user-selected destinations, never a shell cache.
    if (await FileSystemEntity.type(destination.path, followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw const ApiFailure('exists');
    }
    await _exchange(
      path,
      binaryLimit: limit,
      destination: destination,
      isActive: isActive,
    );
  }

  /// WebSocket upgrade over the same owner-only Unix socket. No TCP fallback.
  Future<WebSocket> openMediaSocket(String peer) async {
    if (!RegExp(r'^[a-fA-F0-9]{16}([a-fA-F0-9]{48})?$').hasMatch(peer)) {
      throw const ApiFailure('protocol');
    }
    if (_active.length >= 6) throw const ApiFailure('busy');
    final client = HttpClient()
      ..findProxy = ((_) => 'DIRECT')
      ..connectionTimeout = timeout;
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
    var expired = false;
    final future = WebSocket.connect(
      'ws://localhost/api/media/ws/$peer',
      customClient: client,
    );
    // A handshake completing after timeout/cancel must never escape cleanup.
    future.then((socket) {
      if (expired || !_active.contains(client)) {
        socket.close();
      } else {
        _sockets.add(socket);
        socket.done.whenComplete(() {
          _sockets.remove(socket);
          client.close(force: true);
          _active.remove(client);
        });
      }
    }, onError: (Object _) {});
    try {
      final socket = await future.timeout(timeout);
      if (!_active.contains(client)) {
        await socket.close();
        throw const ApiFailure('disconnected');
      }
      socket.pingInterval = const Duration(seconds: 20);
      return socket;
    } catch (e) {
      expired = true;
      client.close(force: true);
      _active.remove(client);
      if (e is WebSocketException && e.httpStatusCode != null)
        throw ApiFailure('http', e.httpStatusCode);
      if (e is TimeoutException) throw const ApiFailure('timeout');
      throw const ApiFailure('disconnected');
    }
  }

  @override
  void cancel() {
    for (final socket in _sockets.toList()) {
      socket.close();
    }
    _sockets.clear();
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
