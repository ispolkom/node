import 'api_service.dart';
import 'group_api.dart';

/// Группы поверх общего HTTPS-клиента приложения (тот же закреплённый сертификат узла и токен устройства).
class HttpGroupApi implements GroupApi {
  final ApiService _api;
  HttpGroupApi(this._api);

  Future<Map<String, dynamic>> _call(String method, String path, [Object? body]) async {
    final (code, j) = await _api.json(method, path, body);
    if (code != 200) throw GroupApiError(code, j is Map ? '${j['error'] ?? ''}' : '$j');
    return j is Map<String, dynamic> ? j : <String, dynamic>{};
  }

  List<Map<String, dynamic>> _list(Map<String, dynamic> j, String key) => [for (final g in (j[key] as List? ?? const [])) (g as Map).cast<String, dynamic>()];

  @override
  Future<List<Map<String, dynamic>>> list() async => _list(await _call('GET', '/mobile/groups'), 'groups');
  @override
  Future<List<Map<String, dynamic>>> openGroups() async => _list(await _call('GET', '/mobile/groups/open'), 'groups');
  @override
  Future<String> join(String gid, {String? inviteCode, String? memberPub}) async =>
      (await _call('POST', '/mobile/groups/$gid/join', {if (inviteCode != null) 'invite_code': inviteCode, if (memberPub != null) 'member_pub': memberPub}))['alias'] as String;
  @override
  Future<void> leave(String gid) async => _call('POST', '/mobile/groups/$gid/leave', {});
  @override
  Future<Map<String, dynamic>> members(String gid) => _call('GET', '/mobile/groups/$gid/members');
  @override
  Future<void> setPub(String gid, String memberPub) async => _call('POST', '/mobile/groups/$gid/pubkey', {'member_pub': memberPub});
  @override
  Future<Map<String, dynamic>> keys(String gid) => _call('GET', '/mobile/groups/$gid/keys');
  @override
  Future<void> putKeys(String gid, int epoch, Map<String, String> boxes) async => _call('POST', '/mobile/groups/$gid/keys', {'epoch': epoch, 'boxes': boxes});
  @override
  Future<Map<String, dynamic>> pull(String gid, int after, {int limit = 200}) => _call('GET', '/mobile/groups/$gid/log?after=$after&limit=$limit');
  @override
  Future<int> append(String gid, int epoch, String payloadB64) async => (await _call('POST', '/mobile/groups/$gid/log', {'epoch': epoch, 'payload_b64': payloadB64}))['seq'] as int;
  @override
  Future<void> mod(String gid, Map<String, dynamic> body) async => _call('POST', '/mobile/groups/$gid/mod', body);
  @override
  Future<Map<String, dynamic>> invite(String gid, {int uses = 1, int ttlHours = 24}) => _call('POST', '/mobile/groups/$gid/invite', {'uses': uses, 'ttl_hours': ttlHours});
  @override
  Future<List<Map<String, dynamic>>> modlog(String gid) async => _list(await _call('GET', '/mobile/groups/$gid/modlog'), 'modlog');
}
