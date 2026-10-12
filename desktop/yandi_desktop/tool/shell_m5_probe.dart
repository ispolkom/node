// Only run against disposable chaos fixtures.
import '../lib/transport/api.dart';
import '../lib/domain/shell_repository.dart';

Future<void> main(List<String> args) async {
  final api = UnixApi(args[0]);
  try {
    for (final path in ['/api/mobile/devices', '/api/mobile/other-devices']) {
      ShellRepository.list(await api.request(path), 'devices');
    }
    try {
      await api.request(
        '/api/mobile/phone-action',
        body: {'peer': 'd' * 64, 'action': 'invalid'},
      );
      throw StateError('invalid phone action accepted');
    } on ApiFailure catch (e) {
      if (e.status != 400) rethrow;
    }
    final settings = await api.request('/api/settings');
    final updated = await api.request(
      '/api/settings',
      method: 'PUT',
      body: {'network': settings['network'], 'server': settings['server']},
    );
    if (updated['requires_restart'] != true)
      throw StateError('restart banner contract');
    final exit = await api.request('/api/exit/settings');
    await api.request('/api/exit/settings', body: {'mode': 'off'});
    if ((await api.request('/api/exit/settings'))['mode'] != 'off')
      throw StateError('exit mode');
    await api.request('/api/exit/settings', body: {'mode': exit['mode']});
    final vault = await api.request('/api/vault/status');
    if (vault['configured'] != true || vault['unlocked'] != true)
      throw StateError('sealed fixture required');
    await api.request(
      '/api/upstream-proxies',
      body: {
        'kind': 'socks5',
        'host': 'proxy.example.com',
        'port': 1080,
        'auth': 'password',
        'user': 'fixture',
        'password': 'm5-proxy-secret',
      },
    );
    final pool = await api.request('/api/upstream-proxies');
    if (pool.toString().contains('m5-proxy-secret'))
      throw StateError('password leaked');
    final p = ShellRepository.list(pool, 'proxies').single;
    await api.request(
      '/api/upstream-proxies/${p['id']}',
      body: {'enabled': false, 'max_clients': 2},
    );
    await api.request('/api/upstream-proxies/${p['id']}', method: 'DELETE');
    print('PASS: M5 device list and invalid-action rejection via Unix');
  } finally {
    api.cancel();
  }
}
