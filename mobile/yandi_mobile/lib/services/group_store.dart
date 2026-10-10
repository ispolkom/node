import '../models/group.dart';

/// Где группы хранят свои секреты (ключи) и расшифрованные записи. Боевая реализация — защищённое хранилище и база приложения,
/// для тестов — память.
abstract class GroupStore {
  Future<String?> read(String key);
  Future<void> write(String key, String value);
  Future<void> delete(String key);

  /// Записи группы по возрастанию номера; kind — только этого вида.
  Future<List<GroupEntry>> entries(String gid, {int? kind, int? beforeSeq, int limit = 200});
  Future<void> putEntries(List<GroupEntry> list);
  Future<void> markDeleted(String gid, int seq);
  Future<int> lastSeq(String gid);
  Future<void> forgetGroup(String gid);
}

class MemoryGroupStore implements GroupStore {
  final Map<String, String> kv = {};
  final Map<String, Map<int, GroupEntry>> log = {};

  @override
  Future<String?> read(String key) async => kv[key];
  @override
  Future<void> write(String key, String value) async => kv[key] = value;
  @override
  Future<void> delete(String key) async => kv.remove(key);

  @override
  Future<List<GroupEntry>> entries(String gid, {int? kind, int? beforeSeq, int limit = 200}) async {
    var l = (log[gid]?.values.toList() ?? [])..sort((a, b) => a.seq.compareTo(b.seq));
    if (kind != null) l = l.where((e) => e.kind == kind).toList();
    if (beforeSeq != null) l = l.where((e) => e.seq < beforeSeq).toList();
    return l.length > limit ? l.sublist(l.length - limit) : l;
  }

  @override
  Future<void> putEntries(List<GroupEntry> list) async {
    for (final e in list) {
      (log[e.gid] ??= {})[e.seq] = e;
    }
  }

  @override
  Future<void> markDeleted(String gid, int seq) async {
    final e = log[gid]?[seq];
    if (e != null) log[gid]![seq] = e.asDeleted();
  }

  @override
  Future<int> lastSeq(String gid) async => (log[gid]?.keys.fold<int>(0, (a, b) => a > b ? a : b)) ?? 0;

  @override
  Future<void> forgetGroup(String gid) async {
    log.remove(gid);
    kv.removeWhere((k, _) => k.contains('_$gid'));
  }
}
