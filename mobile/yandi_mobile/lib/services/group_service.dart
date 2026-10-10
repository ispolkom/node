import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import 'group_api.dart';
import 'group_store.dart';

/// Отказ отправить запись (запрещённый вид, заглушён и т.п.) — с причиной для человека.
class GroupSendRefused implements Exception {
  final String reason;
  const GroupSendRefused(this.reason);
  @override
  String toString() => reason;
}

/// Работа приложения с группами: ключи, забор журнала, расшифровка, правила содержимого, отправка и модерация.
/// Содержимое группы узел не читает: всё шифруется здесь (`GroupCrypto`).
class GroupService {
  final GroupApi api;
  final GroupStore store;

  /// Вызывается, когда в группе появилось что-то новое (запись, состав, правила).
  final void Function(String gid)? onChanged;

  GroupService(this.api, this.store, {this.onChanged});

  List<GroupInfo> groups = const [];
  final Map<String, GroupRules> _rules = {};
  final Map<String, Map<String, String>> _names = {};       // группа → псевдоним → имя, которое человек назвал при вступлении
  final Set<String> _problems = {};                         // группы, где ключ владельца не совпал с закреплённым
  bool _syncing = false;

  GroupRules rules(String gid) => _rules[gid] ?? const GroupRules();
  String displayName(String gid, String alias) => _names[gid]?[alias] ?? alias;
  bool hasProblem(String gid) => _problems.contains(gid);

  static String _b64(List<int> b) => base64.encode(b);
  static Uint8List _unb64(String s) => Uint8List.fromList(base64.decode(s));

  // ── ключи ────────────────────────────────────────────────────────────────

  /// Мой ключ для этой группы (создаётся при первой надобности; закрытая часть остаётся на телефоне).
  Future<(Uint8List, Uint8List)> _memberKeys(String gid) async {
    final raw = await store.read('grp_mine_$gid');
    if (raw != null) {
      final j = jsonDecode(raw) as Map<String, dynamic>;
      return (_unb64(j['s'] as String), _unb64(j['p'] as String));
    }
    final (seed, pub) = await GroupCrypto.newMemberKeys();
    await store.write('grp_mine_$gid', jsonEncode({'s': _b64(seed), 'p': _b64(pub)}));
    return (seed, pub);
  }

  Future<Uint8List?> _groupKey(String gid, int epoch) async {
    final s = await store.read('grp_key_${epoch}_$gid');
    return s == null ? null : _unb64(s);
  }

  Future<void> _saveGroupKey(String gid, int epoch, List<int> key) => store.write('grp_key_${epoch}_$gid', _b64(key));

  /// Мой ключ опубликован на узле?
  Future<void> _publishPub(String gid) async {
    if (await store.read('grp_pubsent_$gid') == '1') return;
    final (_, pub) = await _memberKeys(gid);
    await api.setPub(gid, _b64(pub));
    await store.write('grp_pubsent_$gid', '1');
  }

  /// Владелец: раздаёт ключ группы. Новое поколение — при `rekey_needed` и самом первом разе; иначе только тем, у кого конверта ещё нет.
  Future<void> _provision(GroupInfo g) async {
    final (ownerSeed, _) = await _memberKeys(g.id);
    final m = await api.members(g.id);
    final members = [for (final x in (m['members'] as List? ?? const [])) GroupMember.fromJson((x as Map).cast<String, dynamic>())];
    final others = members.where((x) => x.alias != g.myAlias && x.memberPub != null).toList();
    var epoch = g.epoch;
    Uint8List? key = epoch == 0 ? null : await _groupKey(g.id, epoch);
    List<GroupMember> targets;
    if (key == null || g.rekeyNeeded) {
      epoch = g.epoch + 1;
      key = GroupCrypto.newGroupKey();
      targets = others;
    } else {
      targets = others.where((x) => !x.hasKey).toList();
      if (targets.isEmpty) return;
    }
    final boxes = <String, String>{};
    for (final t in targets) {
      boxes[t.alias] = _b64(await GroupCrypto.sealKey(key, gid: g.id, epoch: epoch, ownerSeed: ownerSeed, memberPub: _unb64(t.memberPub!)));
    }
    await _saveGroupKey(g.id, epoch, key);
    await api.putKeys(g.id, epoch, boxes);
  }

  /// Участник: забирает свои конверты и открывает их ключом владельца. Ключ владельца закрепляется при первом получении (TOFU):
  /// если узел потом подсунет другой, ключи не принимаются и группа помечается проблемной.
  Future<void> _collectKeys(GroupInfo g) async {
    final (mySeed, _) = await _memberKeys(g.id);
    final k = await api.keys(g.id);
    final ownerPub = k['owner_pub'] as String?;
    if (ownerPub == null) return;
    final pinned = await store.read('grp_owner_${g.id}');
    if (pinned == null) {
      await store.write('grp_owner_${g.id}', ownerPub);
    } else if (pinned != ownerPub) {
      _problems.add(g.id);
      return;
    }
    _problems.remove(g.id);
    for (final b in (k['boxes'] as List? ?? const [])) {
      final epoch = (b as Map)['epoch'] as int;
      if (await _groupKey(g.id, epoch) != null) continue;
      try {
        final key = await GroupCrypto.openKey(_unb64(b['box'] as String), gid: g.id, epoch: epoch, mySeed: mySeed, ownerPub: _unb64(ownerPub));
        await _saveGroupKey(g.id, epoch, key);
      } catch (_) {
        // конверт не от этого владельца или повреждён — не принимаем
        _problems.add(g.id);
      }
    }
  }

  // ── журнал ───────────────────────────────────────────────────────────────

  Future<GroupEntry> _decode(GroupInfo g, Map<String, dynamic> e, Uint8List? key) async {
    final seq = e['seq'] as int, ts = (e['ts'] ?? 0) as int, from = (e['from'] ?? '') as String, epoch = (e['epoch'] ?? 0) as int;
    if (e['deleted'] == true || (e['payload_b64'] ?? '') == '') {
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {}, deleted: true);
    }
    try {
      final r = await GroupCrypto.decryptEntry(key!, _unb64(e['payload_b64'] as String), gid: g.id, epoch: epoch, from: from);
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: r.kind, from: from, body: r.body);
    } catch (_) {
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {'op': 'bad'});
    }
  }

  /// Забирает новые записи. Останавливается на записи, для которой ещё нет ключа (придёт позже), чтобы ничего не потерять.
  Future<int> _pull(GroupInfo g) async {
    var got = 0;
    var after = await store.lastSeq(g.id);
    if (g.firstSeq > after + 1) after = g.firstSeq - 1;   // старое на узле уже стёрто
    while (true) {
      final p = await api.pull(g.id, after, limit: 200);
      final list = [for (final x in (p['entries'] as List? ?? const [])) (x as Map).cast<String, dynamic>()];
      if (list.isEmpty) break;
      final batch = <GroupEntry>[];
      var blocked = false;
      for (final e in list) {
        final epoch = (e['epoch'] ?? 0) as int;
        final isDeleted = e['deleted'] == true || (e['payload_b64'] ?? '') == '';
        final key = isDeleted ? null : await _groupKey(g.id, epoch);
        if (!isDeleted && key == null) { blocked = true; break; }
        batch.add(await _decode(g, e, key));
        after = e['seq'] as int;
      }
      await store.putEntries(batch);
      got += batch.length;
      if (blocked || list.length < 200) break;
    }
    return got;
  }

  /// Правила и имена берутся из служебных записей. Правила принимаются только от владельца группы.
  Future<void> _rebuildMeta(GroupInfo g) async {
    final sys = await store.entries(g.id, kind: GroupKind.system, limit: 2000);
    final names = <String, String>{};
    GroupRules? rules;
    final owner = await _ownerAlias(g);
    for (final e in sys) {
      if (e.deleted) continue;
      final op = e.body['op'];
      if (op == 'name' && e.body['n'] is String) names[e.from] = (e.body['n'] as String).trim();
      if (op == 'rules' && owner != null && e.from == owner) rules = GroupRules.fromBody(e.body);
    }
    _names[g.id] = names;
    if (rules != null) _rules[g.id] = rules;
  }

  Future<String?> _ownerAlias(GroupInfo g) async {
    if (g.isOwner) return g.myAlias;
    final c = await store.read('grp_owneralias_${g.id}');
    if (c != null) return c;
    final m = await api.members(g.id);
    for (final x in (m['members'] as List? ?? const [])) {
      if ((x as Map)['role'] == 'owner') {
        await store.write('grp_owneralias_${g.id}', x['alias'] as String);
        return x['alias'] as String;
      }
    }
    return null;
  }

  // ── общий обход ──────────────────────────────────────────────────────────

  /// Список групп + ключи + новые записи. Безопасно вызывать по намёку из сокета и по таймеру; параллельные вызовы схлопываются.
  Future<void> sync({String? only}) async {
    if (_syncing) return;
    _syncing = true;
    try {
      final list = [for (final j in await api.list()) GroupInfo.fromJson(j)];
      groups = list;
      var relist = false;
      for (final g in list) {
        if (only != null && g.id != only) continue;
        try {
          await _publishPub(g.id);
          if (g.isOwner) {
            await _provision(g);
            relist = true;   // номер поколения мог вырасти: список должен показывать актуальный, иначе отправка решит, что ключа нет
          } else {
            await _collectKeys(g);
          }
          final n = await _pull(g);
          if (n > 0 || !_rules.containsKey(g.id)) await _rebuildMeta(g);
          if (n > 0) onChanged?.call(g.id);
        } on GroupApiError {
          // одна группа недоступна (например, меня выгнали) — остальные продолжают
        }
      }
      if (relist) groups = [for (final j in await api.list()) GroupInfo.fromJson(j)];
    } finally {
      _syncing = false;
    }
  }

  /// Намёк узла «запись №seq изменена» (например, стёрта модератором): перечитать именно её.
  Future<void> refreshEntry(String gid, int seq) async {
    final g = byId(gid);
    if (g == null) return;
    final p = await api.pull(gid, seq - 1, limit: 1);
    for (final x in (p['entries'] as List? ?? const [])) {
      final e = (x as Map).cast<String, dynamic>();
      if (e['seq'] != seq) continue;
      if (e['deleted'] == true || (e['payload_b64'] ?? '') == '') {
        await store.markDeleted(gid, seq);
        onChanged?.call(gid);
      }
    }
  }

  GroupInfo? byId(String gid) {
    for (final g in groups) { if (g.id == gid) return g; }
    return null;
  }

  // ── действия ─────────────────────────────────────────────────────────────

  /// Создать группу: этот телефон — владелец, ключ и права раздаёт он. kind: `invite` или `open`.
  Future<String> createGroup(String name, String topic, String kind) async {
    final gid = await api.create(name.trim(), topic.trim(), kind);
    groups = [for (final j in await api.list()) GroupInfo.fromJson(j)];
    await sync(only: gid);   // сразу создаёт первое поколение ключа
    groups = [for (final j in await api.list()) GroupInfo.fromJson(j)];
    return gid;
  }

  /// Вступить: открытая группа или по коду приглашения. После вступления имя пишется в журнал зашифрованной записью.
  Future<void> join(String gid, {String? inviteCode, required String displayName}) async {
    final (_, pub) = await _memberKeys(gid);
    await api.join(gid, inviteCode: inviteCode, memberPub: _b64(pub));
    await store.write('grp_pubsent_$gid', '1');
    groups = [for (final j in await api.list()) GroupInfo.fromJson(j)];
    // имя можно написать только когда есть ключ; его пришлёт владелец — пишем при первой же синхронизации с ключом
    await store.write('grp_wantname_$gid', displayName);
  }

  Future<void> _announceNameIfNeeded(GroupInfo g) async {
    final want = await store.read('grp_wantname_${g.id}');
    if (want == null || want.isEmpty) return;
    final key = await _groupKey(g.id, g.epoch);
    if (key == null) return;
    await _append(g, GroupKind.system, {'op': 'name', 'n': want}, key);
    await store.delete('grp_wantname_${g.id}');
  }

  Future<void> syncAndAnnounce() async {
    await sync();
    for (final g in groups) {
      try { await _announceNameIfNeeded(g); } catch (_) {}
    }
  }

  Future<int> _append(GroupInfo g, int kind, Map<String, dynamic> body, Uint8List key) async {
    final payload = await GroupCrypto.encryptEntry(key, gid: g.id, epoch: g.epoch, from: g.myAlias, kind: kind, body: body);
    return api.append(g.id, g.epoch, _b64(payload));
  }

  /// Отправить запись. Запрещённый правилами группы вид отвергается ещё здесь; получатели тоже скрывают такие записи.
  Future<int> send(String gid, int kind, Map<String, dynamic> body) async {
    final g = byId(gid);
    if (g == null) throw const GroupSendRefused('Группа не найдена');
    if (g.muted) throw const GroupSendRefused('Вы заглушены в этой группе');
    if (kind != GroupKind.system && !rules(gid).allows(kind)) throw const GroupSendRefused('В этой группе запрещён такой вид сообщений');
    final key = await _groupKey(gid, g.epoch);
    if (key == null) throw const GroupSendRefused('Ключ группы ещё не получен — подождите владельца');
    return _append(g, kind, body, key);
  }

  /// Владелец: запретить виды записей в группе (служебная запись, видна только участникам).
  Future<void> setRules(String gid, GroupRules r) async {
    final g = byId(gid);
    if (g == null || !g.isOwner) throw const GroupSendRefused('Правила задаёт только владелец группы');
    final key = await _groupKey(gid, g.epoch);
    if (key == null) throw const GroupSendRefused('Ключ группы не создан');
    await _append(g, GroupKind.system, r.toBody(), key);
    _rules[gid] = r;
    onChanged?.call(gid);
  }

  /// Запись видна читателю? Служебные и стёртые не показываются как сообщения; запрещённые правилами — скрыты.
  bool visible(String gid, GroupEntry e) => !e.deleted && e.kind != GroupKind.system && rules(gid).allows(e.kind);

  Future<List<GroupEntry>> messages(String gid, {int? kind, int limit = 200}) async {
    final all = await store.entries(gid, kind: kind, limit: limit);
    return all.where((e) => visible(gid, e)).toList();
  }

  Future<void> leave(String gid) async {
    await api.leave(gid);
    await store.forgetGroup(gid);
    groups = groups.where((g) => g.id != gid).toList();
  }

  Future<void> moderate(String gid, String action, {String? alias, int? seq, String? reason}) async {
    await api.mod(gid, {'action': action, if (alias != null) 'alias': alias, if (seq != null) 'seq': seq, if (reason != null) 'reason': reason});
    if (action == 'delete_entry' && seq != null) await store.markDeleted(gid, seq);
  }

  Future<String> newInvite(String gid, {int uses = 1, int ttlHours = 24}) async => (await api.invite(gid, uses: uses, ttlHours: ttlHours))['code'] as String;
}
