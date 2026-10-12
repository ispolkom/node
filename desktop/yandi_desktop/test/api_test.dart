import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'package:yandi_desktop/domain/states.dart';

void main() {
  late Directory temp;
  late HttpServer server;
  late UnixApi api;
  setUp(() async {
    temp = await Directory.systemTemp.createTemp('yandi-api-');
    final path = '${temp.path}/ui.sock';
    server = await HttpServer.bind(
      InternetAddress(path, type: InternetAddressType.unix),
      0,
    );
    api = UnixApi(
      path,
      timeout: const Duration(milliseconds: 150),
      maxResponseBytes: 256,
    );
  });
  tearDown(() async {
    api.cancel();
    await server.close(force: true);
    await temp.delete(recursive: true);
  });

  test('Unix JSON POST: no cookie, authorization or proxy', () async {
    server.listen((req) async {
      expect(req.uri.path, '/api/vault/unlock');
      expect(req.headers.value('cookie'), isNull);
      expect(req.headers.value('authorization'), isNull);
      expect(jsonDecode(await utf8.decoder.bind(req).join()), {
        'password': 'test secret',
      });
      req.response.headers.contentType = ContentType.json;
      req.response.write('{"ok":true,"status":{"configured":true}}');
      await req.response.close();
    });
    expect(
      (await api.request(
        '/api/vault/unlock',
        body: {'password': 'test secret'},
      ))['ok'],
      true,
    );
  });
  test('PUT PATCH DELETE use exact method and JSON body over Unix', () async {
    final seen = <String>[];
    server.listen((req) async {
      seen.add(req.method);
      expect(jsonDecode(await utf8.decoder.bind(req).join()), {'id': 'test'});
      req.response.headers.contentType = ContentType.json;
      req.response.write('{"ok":true}');
      await req.response.close();
    });
    for (final method in ['PUT', 'PATCH', 'DELETE']) {
      await api.request('/api/contacts', method: method, body: {'id': 'test'});
    }
    expect(seen, ['PUT', 'PATCH', 'DELETE']);
    await expectLater(
      api.request('/api/contacts', method: 'CONNECT'),
      throwsA(isA<ApiFailure>()),
    );
    await expectLater(
      api.request('/api/chat/history/%2e%2e'),
      throwsA(isA<ApiFailure>()),
    );
    await expectLater(
      api.request('/api/chat/history/a%2fb'),
      throwsA(isA<ApiFailure>()),
    );
  });
  test('chunked oversized response is rejected', () async {
    server.listen((req) async {
      req.response.headers.contentType = ContentType.json;
      req.response.write(' ' * 257);
      await req.response.close();
    });
    await expectLater(
      api.request('/api/status'),
      throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'large')),
    );
  });
  test('known content length is rejected', () async {
    server.listen((req) async {
      req.response.headers.contentType = ContentType.json;
      req.response.contentLength = 300;
      req.response.write(' ' * 300);
      await req.response.close();
    });
    await expectLater(
      api.request('/api/status'),
      throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'large')),
    );
  });
  test('deadline covers response headers', () async {
    server.listen((_) {});
    await expectLater(
      api.request('/api/status'),
      throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'timeout')),
    );
  });
  test('deadline covers stalled response body', () async {
    server.listen((req) async {
      req.response.headers.contentType = ContentType.json;
      req.response.write('{');
      await req.response.flush();
    });
    await expectLater(
      api.request('/api/status'),
      throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'timeout')),
    );
  });
  for (final code in [200, 401, 429, 501, 302]) {
    test('HTTP $code: safe errors, no secret reflected', () async {
      server.listen((req) async {
        req.response.statusCode = code;
        req.response.headers.contentType = ContentType.json;
        if (code == 302)
          req.response.headers.set('location', 'http://invalid.example/');
        req.response.write('{"status":"error","message":"test secret"}');
        await req.response.close();
      });
      await expectLater(
        api.request('/api/status'),
        throwsA(
          isA<ApiFailure>().having(
            (e) => e.toString().contains('test secret'),
            'redacted',
            false,
          ),
        ),
      );
    });
  }
  for (final body in [
    '<html>secret</html>',
    '[]',
    '{"ok":false}',
    '{"error":"secret"}',
  ]) {
    test('invalid/error envelope $body', () async {
      server.listen((req) async {
        req.response.headers.contentType = ContentType.json;
        req.response.write(body);
        await req.response.close();
      });
      await expectLater(api.request('/api/status'), throwsA(isA<ApiFailure>()));
    });
  }
  test('cancellation aborts pending IO', () async {
    final received = Completer<void>();
    server.listen((_) => received.complete());
    final pending = api.request('/api/status');
    final assertion = expectLater(pending, throwsA(isA<ApiFailure>()));
    await received.future;
    api.cancel();
    await assertion;
  });
  test('missing socket never falls back to TCP', () async {
    await expectLater(
      UnixApi('${temp.path}/missing').request('/api/status'),
      throwsA(isA<ApiFailure>()),
    );
  });
  test('binary Unix download refuses overwrite and cleans partial cancellation', () async {
    var active = true;
    server.listen((req) async {
      req.response.add([1, 2, 3]);
      await req.response.close();
    });
    final target = File('${temp.path}/received');
    await api.download('/api/files/content/file_1/name', target,
        limit: 3, isActive: () => active);
    expect(await target.readAsBytes(), [1, 2, 3]);
    await expectLater(api.download('/api/files/content/file_1/name', target,
        limit: 3, isActive: () => active), throwsA(isA<ApiFailure>()));
    active = false;
    final cancelled = File('${temp.path}/cancelled');
    await expectLater(api.download('/api/files/content/file_1/name', cancelled,
        limit: 3, isActive: () => active), throwsA(isA<ApiFailure>()));
    expect(await cancelled.exists(), false);
    await expectLater(api.bytes('/api/files/content/file_1/name', limit: 2),
        throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'large')));
  });
  test('mutable secret is zeroed', () {
    final secret = SecretBytes('секрет');
    secret.clear();
    expect(secret.bytes.every((byte) => byte == 0), true);
  });
  test('socket resolution mirrors data_dir, CLI overrides env', () {
    expect(
      resolveSocket([], {'HOME': '/h'}),
      '/h/.local/share/yandi/run/yandi-ui.sock',
    );
    expect(
      resolveSocket([], {'HOME': '/h', 'XDG_DATA_HOME': '/d'}),
      '/d/yandi/run/yandi-ui.sock',
    );
    expect(
      resolveSocket([], {'HOME': '/h', 'XDG_DATA_HOME': ''}),
      '/h/.local/share/yandi/run/yandi-ui.sock',
    );
    expect(resolveSocket(['--socket', '/t'], {'YANDI_UI_SOCKET': '/e'}), '/t');
    expect(resolveSocket(['--socket=/t'], {}), '/t');
    expect(resolveSocket([], {'YANDI_UI_SOCKET': '/e'}), '/e');
    expect(() => resolveSocket(['--socket'], {}), throwsFormatException);
    expect(
      () => resolveSocket(['--socket=http://evil'], {}),
      throwsFormatException,
    );
  });
  test('strict startup states and vault migration/error', () {
    expect(
      parseAuth({'setup_required': true, 'recovery_required': false}),
      AuthStage.setup,
    );
    expect(
      parseAuth({'setup_required': false, 'recovery_required': true}),
      AuthStage.recovery,
    );
    expect(
      parseAuth({'setup_required': false, 'recovery_required': false}),
      AuthStage.login,
    );
    expect(() => parseAuth({}), throwsFormatException);
    final v = VaultState.fromJson({
      'configured': true,
      'unlocked': false,
      'auto_lock_secs': 60,
      'migration_pending': true,
      'error': 'bad',
    });
    expect(v.hasError && v.migrationPending && !v.unlocked, true);
  });
}
