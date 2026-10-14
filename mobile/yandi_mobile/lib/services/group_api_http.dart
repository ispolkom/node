import 'dart:typed_data';
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
  Future<String> create(String name, String topic, String about, String kind, {String? logoB64}) async =>
      (await _call('POST', '/mobile/groups', {'name': name, 'topic': topic, 'about': about, 'kind': kind, if (logoB64 != null && logoB64.isNotEmpty) 'logo_b64': logoB64}))['id'] as String;
  @override
  Future<void> settings(String gid, {String? name, String? topic, String? about, String? logoB64}) async =>
      _call('POST', '/mobile/groups/$gid/settings', {if (name != null) 'name': name, if (topic != null) 'topic': topic, if (about != null) 'about': about, if (logoB64 != null) 'logo_b64': logoB64});
  @override
  Future<Uint8List?> logo(String gid) => _api.getGroupLogo(gid);
  @override
  Future<void> autoClean(String gid, int days) async => _call('POST', '/mobile/groups/$gid/autoclean', {'days': days});
  @override
  Future<Map<String, dynamic>> deleted(String gid, int after) => _call('GET', '/mobile/groups/$gid/deleted?after=$after');
  @override
  Future<void> transferOwner(String gid, String alias) async => _call('POST', '/mobile/groups/$gid/owner', {'alias': alias});
  @override
  Future<void> requestJoin(String gid, String name) async => _call('POST', '/mobile/groups/$gid/request', {'name': name});
  @override
  Future<List<Map<String, dynamic>>> requests(String gid) async => _list(await _call('GET', '/mobile/groups/$gid/requests'), 'requests');
  @override
  Future<void> resolveRequest(String gid, String rid) async => _call('DELETE', '/mobile/groups/$gid/requests/$rid');
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
  Future<void> setCard(String gid, String nick, String avatarB64) async =>
      _call('POST', '/mobile/groups/$gid/profile', {'nick': nick, 'avatar_b64': avatarB64});
  @override
  Future<void> setProof(String gid, String proofB64) async => _call('POST', '/mobile/groups/$gid/proof', {'proof': proofB64});
  @override
  Future<List<int>?> cardAvatar(String gid, String alias) async {
    final (code, b) = await _api.bytes('GET', '/mobile/groups/$gid/members/${Uri.encodeComponent(alias)}/avatar');
    return code == 200 && b.isNotEmpty ? b : null;
  }
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

  @override
  Future<({String fileId, int chunkMax})> fileStart(String gid, int size, int chunks) async {
    final j = await _call('POST', '/mobile/groups/$gid/files', {'file_size': size, 'total_chunks': chunks});
    return (fileId: j['file_id'] as String, chunkMax: (j['chunk_max'] ?? 0) as int);
  }
  @override
  Future<void> filePut(String gid, String fileId, int idx, List<int> bytes) async {
    final (code, _) = await _api.bytes('PUT', '/mobile/groups/$gid/files/$fileId/chunk/$idx', bytes);
    if (code != 200) throw GroupApiError(code, 'кусок $idx не залит');
  }
  @override
  Future<void> fileDone(String gid, String fileId) async => _call('POST', '/mobile/groups/$gid/files/$fileId/done', {});
  @override
  Future<List<int>> fileGet(String gid, String fileId, int idx) async {
    final (code, b) = await _api.bytes('GET', '/mobile/groups/$gid/files/$fileId/chunk/$idx');
    if (code != 200) throw GroupApiError(code, 'кусок $idx не скачан');
    return b;
  }
  @override
  Future<void> fileDelete(String gid, String fileId) async => _call('DELETE', '/mobile/groups/$gid/files/$fileId');
}
