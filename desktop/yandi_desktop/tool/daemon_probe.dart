// Test fixtures only. Never point this destructive probe at an owner's socket.
import 'dart:async';
import 'dart:io';
import '../lib/transport/api.dart';
import '../lib/domain/states.dart';

void check(bool value, String label) {
  if (!value) throw StateError(label);
  stdout.writeln('ok: $label');
}

Future<void> main(List<String> args) async {
  final api = UnixApi(args[0]);
  try {
    if (args[1] == 'restart') {
      final v = await api.request('/api/vault/status');
      check(
        v['configured'] == true && v['unlocked'] == false,
        'restart locked',
      );
      await api.request(
        '/api/vault/unlock',
        body: {'password': 'recovered password'},
      );
      check(
        (await api.request('/api/vault/status'))['unlocked'] == true,
        'restart unlock',
      );
      return;
    }
    check(
      parseAuth(await api.request('/api/auth/state')) == AuthStage.setup,
      'initial setup state',
    );
    await api.request(
      '/api/auth/setup',
      body: {
        'login_password': 'testlogin123',
        'login_password_repeat': 'testlogin123',
        'master_password': 'test master password 123',
        'master_password_repeat': 'test master password 123',
      },
    );
    var ready = false;
    final until = DateTime.now().add(const Duration(seconds: 120));
    while (DateTime.now().isBefore(until)) {
      try {
        ready =
            parseAuth(await api.request('/api/auth/state')) == AuthStage.login;
        if (ready) break;
      } on ApiFailure {
        /* startup GET retry only */
      }
      await Future<void>.delayed(const Duration(milliseconds: 500));
    }
    check(ready, 'setup handover to regular daemon');
    check(
      (await api.request('/api/status'))['short_id'] is String,
      'node status',
    );
    check((await api.request('/api/nodes'))['nodes'] is List, 'nodes');
    check((await api.request('/api/speed'))['rx_speed'] is num, 'speed');
    await api.request(
      '/api/vault/setup',
      body: {
        'password': 'owner password',
        'phrase': 'alpha1 bravo2 charlie3 delta4',
      },
    );
    check(
      (await api.request('/api/vault/status'))['unlocked'] == true,
      'vault setup',
    );
    await api.request('/api/vault/lock', body: {});
    check(
      (await api.request('/api/vault/status'))['unlocked'] == false,
      'vault lock',
    );
    try {
      await api.request(
        '/api/vault/unlock',
        body: {'password': 'wrong password'},
      );
      throw StateError('wrong password accepted');
    } on ApiFailure catch (e) {
      check(e.status == 401, 'wrong password rejected');
    }
    await api.request(
      '/api/vault/unlock',
      body: {'password': 'owner password'},
    );
    await api.request(
      '/api/vault/change_password',
      body: {
        'old_password': 'owner password',
        'new_password': 'second password',
      },
    );
    await api.request('/api/vault/auto_lock', body: {'secs': 1});
    await Future<void>.delayed(const Duration(seconds: 3));
    check(
      (await api.request('/api/vault/status'))['unlocked'] == false,
      'daemon auto-lock (poll does not touch)',
    );
    await api.request(
      '/api/vault/unlock',
      body: {'password': 'second password'},
    );
    await api.request('/api/vault/auto_lock', body: {'secs': 0});
    await api.request('/api/vault/lock', body: {});
    await api.request(
      '/api/vault/recover',
      body: {
        'phrase': 'alpha1 bravo2 charlie3 delta4',
        'new_password': 'recovered password',
      },
    );
    check(
      (await api.request('/api/vault/status'))['unlocked'] == true,
      'phrase recovery',
    );
    for (final path in ['/api/node/start', '/api/gateway/start']) {
      try {
        await api.request(path, body: {});
        throw StateError('expected 501');
      } on ApiFailure catch (e) {
        check(e.status == 501, 'unimplemented service command');
      }
    }
  } catch (e) {
    stderr.writeln(e is ApiFailure ? e.message : 'Проверка не пройдена: $e');
    exitCode = 1;
  } finally {
    api.cancel();
  }
}
