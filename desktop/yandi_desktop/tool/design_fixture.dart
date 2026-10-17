import 'package:yandi_desktop/transport/api.dart';

/// Deterministic, memory-only data for design tests and Linux previews.
/// Never instantiated by the production entrypoint.
class DesignApi implements Api {
  final String scene;
  DesignApi([this.scene = 'Главная']);
  static final peer = 'a' * 64, group = 'b' * 32;
  static const short = 'aaaaaaaaaaaaaaaa';
  @override
  void cancel() {}
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (scene == 'offline') throw const ApiFailure('disconnected');
    if (path == '/api/auth/seed/new')
      return {
        'count': 12,
        'min_password': 6,
        'words': 'legal winner thank year wave sausage worth useful legal winner thank yellow'.split(' '),
      };
    if (path == '/api/auth/state')
      return {'setup_required': scene == 'setup', 'recovery_required': false};
    if (path == '/api/vault/status')
      return {
        'configured': true,
        'unlocked': scene != 'unlock',
        'has_error': false,
        'auto_lock_secs': 900,
        'migration_pending': false,
      };
    if (path == '/api/status')
      return {
        'is_local': true,
        'status': 'online',
        'short_id': short,
        'node_id': 'c' * 64,
        'cid': 'YANDI-NODE-1',
        'role': 'peer',
        'mode': 'p2p',
        'nat_status': 'Open',
        'external_ip': '192.0.2.10',
        'virtual_ipv6': 'fd00::1',
        'ipv6_short': '::1',
        'discovery_port': 9000,
        'data_port': 10000,
        'web_port': 8080,
        'active_proxies': 0,
      };
    if (path == '/api/speed')
      return {'rx_speed': 2048, 'tx_speed': 1024, 'avg_rtt_ms': 18};
    if (path == '/api/nodes')
      return {
        'nodes': [
          {
            'short_id': short,
            'hostname': 'Алиса',
            'source': 'p2p',
            'role': 'peer',
            'connected': true,
          },
        ],
      };
    if (path == '/api/contacts')
      return {
        'contacts': [
          {'id': 'alice', 'short_id': short, 'name': 'Алиса', 'online': true},
          {
            'id': 'bob',
            'short_id': 'dddddddddddddddd',
            'name': 'Боб',
            'online': false,
          },
        ],
      };
    if (path == '/api/peers/trusted')
      return {
        'card': 'YANDI-NODE-1:design-fixture',
        'peers': [
          {'id': peer},
        ],
      };
    if (path.startsWith('/api/profile/'))
      return {
        'profile': {
          'display_name': path.endsWith('/me') ? 'Мой профиль' : 'Алиса',
          'short_id': short,
          'online': true,
        },
      };
    if (path.startsWith('/api/chat/history/'))
      return {
        'messages': [
          {
            'msg_id': '1',
            'from': peer,
            'text': 'Привет! Как дела?',
            'timestamp': 4102444800000,
            'status': 'Sent',
          },
          {
            'msg_id': '2',
            'from': 'c' * 64,
            'text': 'Привет! Всё хорошо.',
            'timestamp': 4102444860000,
            'status': 'Delivered',
          },
        ],
      };
    if (path == '/api/mobile/devices' || path == '/api/mobile/other-devices')
      return {'devices': []};
    if (path == '/api/settings')
      return {
        'network': {
          'discovery_port': 9000,
          'data_port': 10000,
          'mobile_gateway': 8443,
          'mobile_p2p': 8444,
          'http_proxy': 8081,
          'web_ui': 8080,
        },
        'server': {'bind_address': '127.0.0.1', 'log_level': 'info'},
      };
    if (path == '/api/exit/settings') return {'mode': 'trusted'};
    if (path == '/api/upstream-proxies')
      return {
        'max_clients': 4,
        'enabled': false,
        'strict': true,
        'proxies': [],
      };
    if (path == '/api/gateways')
      return {
        'gateways': [
          {
            'id': 'gateway1',
            'short_id': short,
            'name': 'Домашний шлюз',
            'country': 'RU',
            'connected': true,
          },
        ],
      };
    if (path == '/api/proxy/status' || path == '/api/socks5/status')
      return {'active': false, 'proxies': []};
    if (path == '/api/relay/status')
      return {
        'relay_server': false,
        'active_sessions': 0,
        'total_bytes_forwarded': 0,
        'total_packets_forwarded': 0,
      };
    if (path == '/api/relay/sessions') return {'sessions': []};
    final entry = {
      'id': group,
      'name': 'Семья',
      'role': 'owner',
      'kind': 'invite',
      'topic': 'Общение',
      'about': 'Семейная группа',
      'muted': false,
    };
    if (path == '/api/groups')
      return {
        'groups': [entry],
        'catalog': [],
        'gone': [],
      };
    if (path == '/api/mobile/groups')
      return {
        'groups': [entry],
      };
    if (path.endsWith('/messages'))
      return {
        'group': entry,
        'has_key': true,
        'deny': [],
        'messages': [
          {
            'seq': 1,
            'ts': 4102444800000,
            'from': 'alice',
            'author': 'Алиса',
            'kind': 1,
            'body': {'t': 'Всем привет!'},
          },
        ],
      };
    if (path.endsWith('/info'))
      return {
        'group': entry,
        'members': [],
        'requests': [],
        'banned': [],
        'modlog': [],
        'deny': [],
      };
    if (path.startsWith('/api/mobile/call/events'))
      return {'last': 0, 'events': []};
    if (path.startsWith('/api/mobile/call/') || path.startsWith('/api/call/'))
      return {'signals': [], 'calls': []};
    if (path.contains('tunnel'))
      return {
        'tunnel': {
          'tunnel_status': 'Inactive',
          'tunnel_type': 'Generic',
          'bytes_sent': 0,
          'bytes_received': 0,
        },
      };
    return {'ok': true};
  }
}
