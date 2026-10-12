// Only invoked by the isolated chaos fixture with two disposable /tmp nodes.
import '../lib/transport/api.dart';
import '../lib/domain/network_repository.dart';

Future<void> main(List<String> args) async {
  final api = UnixApi(args[0]), peer = UnixApi(args[1]);
  final repo = NetworkRepository(api);
  try {
    final short = (await peer.request('/api/status'))['short_id'] as String;
    await repo.save('Тестовый шлюз', short, 'RU');
    final saved = (await repo.gateways()).singleWhere(
      (r) => r['short_id'] == short,
    );
    await repo.save('Изменённый шлюз', short, 'NL', key: saved['id'] as String);
    await repo.server(true);
    await repo.connect(short);
    await repo.sessions();
    await repo.server(false);
    for (final family in ['proxy', 'socks5']) {
      await repo.proxy(family);
      await repo.proxyAction(family, 'start', short);
      final status = await repo.proxy(family);
      if (status['active'] != true) throw StateError('proxy metadata start');
      await repo.proxyAction(family, 'stop', short);
      if ((await repo.proxy(family))['active'] != false)
        throw StateError('proxy metadata stop');
    }
    await repo.remove(saved['id'] as String);
    print(
      'PASS: gateway CRUD, HTTP/SOCKS metadata, relay lifecycle/connect via Unix',
    );
  } finally {
    api.cancel();
    peer.cancel();
  }
}
