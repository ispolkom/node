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
  Future<String> create(String name, String topic, String about, String kind);
  Future<void> requestJoin(String gid, String name);
  Future<void> settings(String gid, {String? name, String? topic, String? about});
  /// Владелец включает автоочистку неактивных (0 — выключить).
  Future<void> autoClean(String gid, int days);
  Future<void> transferOwner(String gid, String alias);
  /// Стирания после курсора: `{deleted:[seq], cursor, resync}`.
  Future<Map<String, dynamic>> deleted(String gid, int after);
  Future<List<Map<String, dynamic>>> requests(String gid);
  Future<void> resolveRequest(String gid, String rid);
  Future<String> join(String gid, {String? inviteCode, String? memberPub});
  Future<void> leave(String gid);
  Future<Map<String, dynamic>> members(String gid);
  Future<void> setPub(String gid, String memberPub);
  /// Раскрыть свой ник и картинку в группе (пустые — убрать).
  Future<void> setCard(String gid, String nick, String avatarB64);
  /// Приложить подпись «это я» (пустая — убрать): проверить её могут только контакты подписавшего.
  Future<void> setProof(String gid, String proofB64);
  /// Картинка визитки участника (PNG) по псевдониму; null — нет визитки или картинки.
  Future<List<int>?> cardAvatar(String gid, String alias);
  Future<Map<String, dynamic>> keys(String gid);
  Future<void> putKeys(String gid, int epoch, Map<String, String> boxes);
  Future<Map<String, dynamic>> pull(String gid, int after, {int limit = 200});
  Future<int> append(String gid, int epoch, String payloadB64);
  Future<void> mod(String gid, Map<String, dynamic> body);
  Future<Map<String, dynamic>> invite(String gid, {int uses = 1, int ttlHours = 24});
  Future<List<Map<String, dynamic>>> modlog(String gid);

  // файлы группы (шифртекст кусками)
  Future<({String fileId, int chunkMax})> fileStart(String gid, int size, int chunks);
  Future<void> filePut(String gid, String fileId, int idx, List<int> bytes);
  Future<void> fileDone(String gid, String fileId);
  Future<List<int>> fileGet(String gid, String fileId, int idx);
  Future<void> fileDelete(String gid, String fileId);
}
