import 'dart:convert';
import 'package:sqflite/sqflite.dart';
import '../models/group.dart';
import 'group_store.dart';
import 'storage_service.dart';

/// Боевое хранилище групп: ключи — в защищённом хранилище Android, расшифрованные записи — в базе приложения.
class SqliteGroupStore implements GroupStore {
  Database get _db => StorageService.db;

  @override
  Future<String?> read(String key) => StorageService.secureRead(key);
  @override
  Future<void> write(String key, String value) => StorageService.secureWrite(key, value);
  @override
  Future<void> delete(String key) => StorageService.secureDelete(key);

  GroupEntry _row(Map<String, Object?> r) => GroupEntry(
        gid: r['gid'] as String,
        seq: r['seq'] as int,
        ts: r['ts'] as int,
        epoch: r['epoch'] as int,
        kind: r['kind'] as int,
        from: r['sender'] as String,
        body: (jsonDecode(r['body'] as String) as Map).cast<String, dynamic>(),
        deleted: (r['deleted'] as int) == 1,
      );

  @override
  Future<List<GroupEntry>> entries(String gid, {int? kind, int? beforeSeq, int limit = 200}) async {
    final where = StringBuffer('gid = ?');
    final args = <Object?>[gid];
    if (kind != null) { where.write(' AND kind = ?'); args.add(kind); }
    if (beforeSeq != null) { where.write(' AND seq < ?'); args.add(beforeSeq); }
    final rows = await _db.query('group_entries', where: where.toString(), whereArgs: args, orderBy: 'seq DESC', limit: limit);
    return rows.map(_row).toList().reversed.toList();
  }

  @override
  Future<void> putEntries(List<GroupEntry> list) async {
    final b = _db.batch();
    for (final e in list) {
      b.insert('group_entries', {
        'gid': e.gid, 'seq': e.seq, 'ts': e.ts, 'epoch': e.epoch, 'kind': e.kind, 'sender': e.from, 'body': e.bodyJson, 'deleted': e.deleted ? 1 : 0,
      }, conflictAlgorithm: ConflictAlgorithm.replace);
    }
    await b.commit(noResult: true);
  }

  @override
  Future<void> markDeleted(String gid, int seq) =>
      _db.update('group_entries', {'body': '{}', 'deleted': 1}, where: 'gid = ? AND seq = ?', whereArgs: [gid, seq]);

  @override
  Future<int> lastSeq(String gid) async {
    final r = await _db.rawQuery('SELECT MAX(seq) AS m FROM group_entries WHERE gid = ?', [gid]);
    return (r.first['m'] as int?) ?? 0;
  }

  @override
  Future<void> forgetGroup(String gid) async {
    await _db.delete('group_entries', where: 'gid = ?', whereArgs: [gid]);
    for (final k in ['grp_mine_$gid', 'grp_pubsent_$gid', 'grp_owner_$gid', 'grp_owneralias_$gid', 'grp_wantname_$gid', 'grp_ignore_$gid']) {
      await StorageService.secureDelete(k);
    }
    for (var e = 0; e < 64; e++) { await StorageService.secureDelete('grp_key_${e}_$gid'); }
  }
}
