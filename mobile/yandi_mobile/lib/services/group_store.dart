import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../models/tombstone.dart';

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
  /// Есть ли уже запись с таким внутренним номером (защита от повторов).
  Future<bool> hasEntryId(String gid, String id);
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
  Future<bool> hasEntryId(String gid, String id) async => (log[gid]?.values ?? const <GroupEntry>[]).any((e) => e.body['i'] == id);

  @override
  Future<void> forgetGroup(String gid) async {
    log.remove(gid);
    kv.removeWhere((k, _) => secureKeyOfGroup(k, gid));
  }
}

/// Ключ защищённого хранилища относится к группе gid: `grp_…_<gid>` или `grp_…_<gid>_…`. Номер группы — 32 hex-знака, так что
/// чужие группы не задеваются.
bool secureKeyOfGroup(String key, String gid) => key.startsWith('grp_') && (key.endsWith('_$gid') || key.contains('_${gid}_'));

/// Запись — надгробие «удалено вместе с пользователем».
bool isTombstoneEntry(GroupEntry e) => !e.deleted && e.kind == GroupKind.text && e.body['t'] == Tombstone.marker;

/// Записи `seqs` — удалены вместе с пользователем: остаются номер и время, вид — текст, тело — метка (текст, вложение, правки пропадают).
/// Цитаты на них в других записях получают метку вместо выдержки, смайлы-голоса на них и голоса их авторов стираются (строка остаётся
/// стёртой, чтобы курсор ленты не откатился). Записей, которых в базе ещё нет, не касается: они придут с пометкой «gone» и будут надгробиями сразу.
Future<void> tombstoneEntries(GroupStore store, String gid, Set<int> seqs) async {
  if (seqs.isEmpty) return;
  final all = await store.entries(gid, limit: 50000);
  final authors = {for (final e in all) if (seqs.contains(e.seq) && e.from.isNotEmpty) e.from};
  final out = <GroupEntry>[];
  for (final e in all) {
    if (seqs.contains(e.seq)) {
      if (!isTombstoneEntry(e) || e.body.length != 1) {
        out.add(GroupEntry(gid: gid, seq: e.seq, ts: e.ts, epoch: e.epoch, kind: GroupKind.text, from: e.from, body: const {'t': Tombstone.marker}));
      }
      continue;
    }
    if (e.kind == GroupKind.reaction) {
      if (!e.deleted && (authors.contains(e.from) || seqs.contains(e.body['s']))) out.add(e.asDeleted());
      continue;
    }
    final r = e.body['r'];
    if (!e.deleted && r is Map && r['s'] is int && seqs.contains(r['s']) && r['t'] != Tombstone.marker) {
      out.add(GroupEntry(gid: gid, seq: e.seq, ts: e.ts, epoch: e.epoch, kind: e.kind, from: e.from, body: {...e.body, 'r': {...r, 't': Tombstone.marker}}));
    }
  }
  if (out.isNotEmpty) await store.putEntries(out);
}
