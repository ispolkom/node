/// Запросы к группам узла (docs/CLIENT_WIRE.md «Группы»). Интерфейс отдельно, чтобы сервис проверялся без сети.
class GroupApiError implements Exception {
  final int status;
  final String message;
  const GroupApiError(this.status, this.message);
  @override
  String toString() => 'GroupApiError($status): $message';
}

abstract class GroupApi {
  Future<List<Map<String, dynamic>>> list();
  Future<List<Map<String, dynamic>>> openGroups();
  Future<String> join(String gid, {String? inviteCode, String? memberPub});
  Future<void> leave(String gid);
  Future<Map<String, dynamic>> members(String gid);
  Future<void> setPub(String gid, String memberPub);
  Future<Map<String, dynamic>> keys(String gid);
  Future<void> putKeys(String gid, int epoch, Map<String, String> boxes);
  Future<Map<String, dynamic>> pull(String gid, int after, {int limit = 200});
  Future<int> append(String gid, int epoch, String payloadB64);
  Future<void> mod(String gid, Map<String, dynamic> body);
  Future<Map<String, dynamic>> invite(String gid, {int uses = 1, int ttlHours = 24});
  Future<List<Map<String, dynamic>>> modlog(String gid);
}
