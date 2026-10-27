import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../models/tombstone.dart';
import '../crypto/identity.dart';
import 'group_api.dart';
import 'group_store.dart';
import '../l10n_core.dart';

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
  final void Function(String gid)? onActivity;
  final Set<String> _soundBaseline = {};

  /// Номер узла, с которым работает сервис: список «известных групп» ведётся отдельно для каждого узла. Иначе переключение на
  /// другой узел (его список групп не содержит групп прежнего) выглядело бы как удаление групп и стирало их ключи и историю.
  final String nodeKey;

  GroupService(this.api, this.store, {this.onChanged, this.onActivity, this.nodeKey = ''});

  bool _retired = false;

  /// Сервис снят с работы (узел сменился): его незавершённая синхронизация больше ничего не стирает.
  void retire() => _retired = true;

  String get _knownKey => nodeKey.isEmpty ? 'grp_known' : 'grp_known_$nodeKey';

  List<GroupInfo> groups = const [];
  final Map<String, GroupRules> _rules = {};
  final Map<String, Map<String, String>> _names = {};       // группа → псевдоним → имя, которое человек назвал при вступлении
  final Map<String, Map<String, String>> _roles = {};       // группа → псевдоним → роль (для проверки «кого я могу стереть»)
  final Set<String> _problems = {};                         // группы, где ключ владельца не совпал с закреплённым
  bool _syncing = false;

  /// Группы, которых узел больше не показывает (удалены на узле или меня выгнали): (номер, название). История у меня ещё лежит.
  List<(String, String)> gone = const [];

  GroupRules rules(String gid) => _rules[gid] ?? const GroupRules();
  final Map<String, Map<String, String>> _nicks = {};       // группа → псевдоним → ник, раскрытый самим участником
  /// Участники, в которых я узнал своего контакта по подписи в визитке: группа → псевдоним → имя контакта (оно только у меня).
  final Map<String, Map<String, String>> _known = {};
  /// Ник, который человек сам раскрыл (он постоянный и следует за его профилем), главнее имени, под которым он записан у меня в контактах.
  /// Имя в книжке главнее ника: если участник — мой контакт (опознан по подписи), я вижу его под тем именем, что дал сам; у остальных — его собственный ник.
  String displayName(String gid, String alias) => _known[gid]?[alias] ?? _nicks[gid]?[alias] ?? _names[gid]?[alias] ?? alias;
  /// true — это мой контакт (узнан по подписи): ему показывается моё имя для него.
  bool isKnownContact(String gid, String alias) => _known[gid]?.containsKey(alias) ?? false;

  /// Подпись личным ключом (ставит приложение); без неё доказательство «это я» не прикладывается.
  Future<List<int>> Function(List<int>)? signer;
  /// Ключи подписи моих контактов (открытый Ed25519, который я уже закрепил при переписке) и имя, которое я им дал.
  Future<List<(List<int>, String, String)>> Function()? contactKeys;   // (ключ подписи, имя в книжке, номер контакта)
  /// Моя локальная картинка контакта (видна только мне), если поставлена: главнее той, что он выложил в группе.
  Uint8List? Function(String peerId)? contactAvatar;
  final Map<String, Map<String, String>> _knownPeer = {};   // группа → псевдоним → номер моего контакта

  static const proofDomain = 'YANDI-GROUP-CARD-V1\u0000';
  static List<int> proofMessage(String gid, List<int> memberPub) => [...utf8.encode(proofDomain), ...utf8.encode(gid), 0, ...memberPub];

  /// Приложить подпись «это я» в этой группе (один раз: ключ группы у меня постоянный).
  Future<void> _ensureProof(String gid) async {
    final sign = signer;
    if (sign == null || await store.read('grp_proof_$gid') == '1') return;
    final (_, pub) = await _memberKeys(gid);
    final sig = await sign(proofMessage(gid, pub));
    await api.setProof(gid, base64.encode(sig));
    await store.write('grp_proof_$gid', '1');
  }

  /// Чьи подписи в группе принадлежат моим контактам. Проверка по всем ключам контактов; посторонний подпись проверить не может.
  Future<void> _recognise(String gid, List members) async {
    final keys = await (contactKeys?.call() ?? Future.value(const <(List<int>, String, String)>[]));
    final found = <String, String>{};
    final foundPeer = <String, String>{};
    for (final x in members) {
      if (x is! Map || x['alias'] is! String || x['proof'] is! String || x['member_pub'] is! String) continue;
      List<int> sig, pub;
      try {
        sig = base64.decode(x['proof'] as String);
        pub = base64.decode(x['member_pub'] as String);
      } catch (_) { continue; }
      if (sig.length != 64) continue;
      final msg = proofMessage(gid, pub);
      for (final (key, name, peer) in keys) {
        if (key.length != 32) continue;
        if (await Identity.verify(message: msg, signature: sig, publicKey: key)) {
          found[x['alias'] as String] = name;
          foundPeer[x['alias'] as String] = peer;
          break;
        }
      }
    }
    _known[gid] = found;
    _knownPeer[gid] = foundPeer;
  }

  /// Мой профиль, из которого берётся визитка (ставит приложение при старте и при смене).
  String _profileNick = '';
  List<int>? _profileAvatar;
  void setProfile(String nick, List<int>? avatar) {
    _profileNick = nick;
    _profileAvatar = avatar;
  }

  /// Отпечаток визитки: пусто, если в профиле нет ни ника, ни картинки.
  String _cardPrint(String nick, List<int>? avatar) {
    if (nick.isEmpty && (avatar == null || avatar.isEmpty)) return '';
    var h = 0x811c9dc5;
    for (final b in utf8.encode(nick)) { h = ((h ^ b) * 0x01000193) & 0xffffffff; }
    for (final b in avatar ?? const <int>[]) { h = ((h ^ b) * 0x01000193) & 0xffffffff; }
    return '${nick.length}:${avatar?.length ?? 0}:$h';
  }

  /// Выложить визитку (ник и картинку из профиля) в группе, если она с прошлого раза изменилась. Ник виден всегда: скрыть его нельзя.
  Future<void> _ensureCard(String gid) async {
    final print = _cardPrint(_profileNick, _profileAvatar);
    if (await store.read('grp_card_fp_$gid') == print) return;
    final small = _profileAvatar;
    await api.setCard(gid, _profileNick, small != null ? base64.encode(small) : '');
    await store.write('grp_card_fp_$gid', print);
  }

  /// Профиль поменялся — обновить визитки во всех группах.
  Future<void> refreshCards(String nick, List<int>? avatar) async {
    setProfile(nick, avatar);
    for (final g in groups) {
      try { await _ensureCard(g.id); } catch (_) {}
    }
  }

  // ── картинки участников ──────────────────────────────────────────────────
  final Map<String, String> _avatarHash = {};       // «группа/псевдоним» → отпечаток, по которому картинка загружена
  final Map<String, Uint8List> _avatars = {};
  final Set<String> _avatarAsked = {};

  /// Картинка участника, раскрытая им в группе; null — нет (или ещё не загрузилась: по приходу вызовется onChanged).
  Uint8List? memberAvatar(String gid, String alias) {
    final peer = _knownPeer[gid]?[alias];
    if (peer != null) {
      final mine = contactAvatar?.call(peer);   // я сам поставил этому контакту картинку — она главнее
      if (mine != null) return mine;
    }
    final key = '$gid/$alias';
    final want = _avatarWant[key];
    if (want != null && want.isNotEmpty && _avatarHash[key] != want && _avatarAsked.add(key)) {
      unawaited(_loadAvatar(gid, alias, key, want));
    }
    return _avatars[key];
  }

  final Map<String, String> _avatarWant = {};      // «группа/псевдоним» → отпечаток картинки у участника (из списка участников)

  Future<void> _loadAvatar(String gid, String alias, String key, String want) async {
    try {
      final b = await api.cardAvatar(gid, alias);
      if (b != null) {
        _avatars[key] = Uint8List.fromList(b);
        _avatarHash[key] = want;
        onChanged?.call(gid);
      }
    } catch (_) {
    } finally {
      _avatarAsked.remove(key);
    }
  }

  bool hasProblem(String gid) => _problems.contains(gid);
  /// Роль участника по псевдониму (null — не знаем или ушёл из группы).
  String? roleOf(String gid, String alias) => _roles[gid]?[alias];

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

  /// Участники, чей ключ изменился после закрепления: им групповой ключ не выдаётся, пока владелец не разберётся.
  final Map<String, Set<String>> _keyConflicts = {};
  Set<String> keyConflicts(String gid) => _keyConflicts[gid] ?? const <String>{};

  /// Владелец: раздаёт ключ группы. Новое поколение — при `rekey_needed` и самом первом разе; иначе только тем, у кого конверта ещё нет.
  Future<void> _provision(GroupInfo g) async {
    final (ownerSeed, _) = await _memberKeys(g.id);
    final m = await api.members(g.id);
    final members = [for (final x in (m['members'] as List? ?? const [])) GroupMember.fromJson((x as Map).cast<String, dynamic>())];
    // ключ участника закрепляется при первом виде: подмена списка узлом не даст получить групповой ключ под чужим ключом
    final others = <GroupMember>[];
    for (final x in members) {
      if (x.alias == g.myAlias || x.memberPub == null) continue;
      final pinKey = 'grp_mpin_${g.id}_${x.alias}';
      final pinned = await store.read(pinKey);
      if (pinned == null) {
        await store.write(pinKey, x.memberPub!);
      } else if (pinned != x.memberPub) {
        _keyConflicts.putIfAbsent(g.id, () => <String>{}).add(x.alias);
        continue;
      }
      others.add(x);
    }
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
      // негодный ключ одного участника не должен останавливать раздачу остальным
      try {
        boxes[t.alias] = _b64(await GroupCrypto.sealKey(key, gid: g.id, epoch: epoch, ownerSeed: ownerSeed, memberPub: _unb64(t.memberPub!)));
      } catch (_) {}
    }
    await _saveGroupKey(g.id, epoch, key);
    await api.putKeys(g.id, epoch, boxes);
  }

  /// Участник: забирает свои конверты и открывает их ключом владельца. Ключ владельца закрепляется при первом получении (TOFU):
  /// если узел потом подсунет другой, ключи не принимаются и группа помечается проблемной.
  /// Забрать свои конверты ключа группы. Каждый конверт запечатан ключом того, кто был владельцем в его поколении (узел отдаёт это для
  /// каждого конверта). Если запечатавший — не тот, чей ключ я закрепил, ищу в ленте запись «владение передано» от закреплённого владельца
  /// с этим новым ключом и иду по цепочке передач (даже если пропустил несколько, пока был не в сети). Без такой записи ключи не принимаю.
  Future<void> _collectKeys(GroupInfo g, {bool asOwner = false}) async {
    final (mySeed, _) = await _memberKeys(g.id);
    final k = await api.keys(g.id);
    final topOwner = k['owner_pub'] as String?;
    var pinned = await store.read('grp_owner_${g.id}');
    if (pinned == null) {
      if (asOwner || topOwner == null) return;
      pinned = topOwner;
      await store.write('grp_owner_${g.id}', pinned);
      // псевдоним закреплённого владельца: нужен, чтобы подтвердить передачу владения, когда он сам уже вышел из группы
      try {
        final ms = (await api.members(g.id))['members'] as List? ?? const [];
        for (final x in ms) { if ((x as Map)['member_pub'] == pinned) { await store.write('grp_owner_alias_${g.id}', '${x['alias']}'); break; } }
      } catch (_) {}
    }
    final boxes = [for (final b in (k['boxes'] as List? ?? const [])) (b as Map).cast<String, dynamic>()]..sort((a, b) => (a['epoch'] as int).compareTo(b['epoch'] as int));
    var problem = false;
    for (var round = 0; round < 8; round++) {
      final missing = <Map<String, dynamic>>[];
      for (final b in boxes) {
        if (await _groupKey(g.id, b['epoch'] as int) == null) missing.add(b);
      }
      if (missing.isEmpty) break;
      var opened = false;
      final wanted = <String>{};
      for (final b in missing) {
        final sealer = (b['owner_pub'] as String?) ?? topOwner;
        if (sealer == null) continue;
        if (sealer != pinned) {
          wanted.add(sealer);
          continue;
        }
        try {
          final epoch = b['epoch'] as int;
          final key = await GroupCrypto.openKey(_unb64(b['box'] as String), gid: g.id, epoch: epoch, mySeed: mySeed, ownerPub: _unb64(pinned!));
          await _saveGroupKey(g.id, epoch, key);
          opened = true;
        } catch (_) {
          problem = true; // конверт не от закреплённого владельца или повреждён — не принимаем
        }
      }
      if (opened) continue; // новые ключи — можно дочитать ленту и найти следующую передачу
      if (wanted.isEmpty) break;
      try { await _pull(g); } catch (_) {}
      final members = [for (final x in ((await api.members(g.id))['members'] as List? ?? const [])) (x as Map).cast<String, dynamic>()];
      final log = await store.entries(g.id, kind: GroupKind.system, limit: 5000);
      final next = handoverNext(log, members, pinned!, wanted, pinnedAlias: await store.read('grp_owner_alias_${g.id}'));
      if (next == null) {
        problem = true;
        break;
      }
      pinned = next.$1;
      await store.write('grp_owner_${g.id}', next.$1);
      if (next.$2 != null) await store.write('grp_owner_alias_${g.id}', next.$2!);
    }
    if (!asOwner) {
      problem ? _problems.add(g.id) : _problems.remove(g.id);
    }
  }

  /// Догнать стирания модераторов, пропущенные без сети (по курсору событий стирания на узле).
  Future<bool> _syncDeletions(GroupInfo g) async {
    final cur = int.tryParse(await store.read('grp_delc_${g.id}') ?? '') ?? 0;
    final r = await api.deleted(g.id, cur);
    var changed = false;
    for (final s in (r['deleted'] as List? ?? const [])) {
      await store.markDeleted(g.id, s as int);
      changed = true;
    }
    // записи удалённых вместе с пользователем: после стираний (надгробие сильнее «стёрто»), тот же курсор
    final forgotten = <int>{for (final s in (r['gone'] as List? ?? const [])) if (s is int) s};
    if (r['resync'] == true) {
      // журнал стираний на узле короче моего пропуска: перечитываю пометки по всей ленте
      var after = g.firstSeq > 0 ? g.firstSeq - 1 : 0;
      while (true) {
        final p = await api.pull(g.id, after, limit: 500);
        final list = [for (final x in (p['entries'] as List? ?? const [])) (x as Map).cast<String, dynamic>()];
        if (list.isEmpty) break;
        for (final e in list) {
          if (e['gone'] == true && e['seq'] is int) { forgotten.add(e['seq'] as int); changed = true; }
          else if (e['deleted'] == true) { await store.markDeleted(g.id, e['seq'] as int); changed = true; }
          after = e['seq'] as int;
        }
        if (list.length < 500) break;
      }
    }
    if (forgotten.isNotEmpty) { await tombstoneEntries(store, g.id, forgotten); changed = true; }
    await store.write('grp_delc_${g.id}', '${r['cursor'] ?? cur}');
    return changed;
  }

  // ── журнал ───────────────────────────────────────────────────────────────

  Future<GroupEntry> _decode(GroupInfo g, Map<String, dynamic> e, Uint8List? key) async {
    final seq = e['seq'] as int;
    final ts = e['ts'] is int ? e['ts'] as int : 0, from = e['from'] is String ? e['from'] as String : '', epoch = e['epoch'] == null ? 0 : (e['epoch'] is int ? e['epoch'] as int : -1);
    if (epoch < 0 || (e['payload_b64'] != null && e['payload_b64'] is! String)) {
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: 0, kind: GroupKind.system, from: from, body: const {'op': 'bad'});
    }
    if (e['deleted'] == true || (e['payload_b64'] ?? '') == '') {
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {}, deleted: true);
    }
    try {
      final r = await GroupCrypto.decryptEntry(key!, _unb64(e['payload_b64'] as String), gid: g.id, epoch: epoch, from: from);
      // подлинная запись, но поля не тех типов (текст — не строка и т. п.): показываем как повреждённую, а не роняем экран
      for (final f in const ['t', 'name', 'n', 'f', 'h', 'k', 'i']) {
        if (r.body.containsKey(f) && r.body[f] != null && r.body[f] is! String) {
          return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {'op': 'bad'});
        }
      }
      for (final f in const ['c', 'z']) {
        if (r.body.containsKey(f) && r.body[f] is! int) {
          return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {'op': 'bad'});
        }
      }
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: r.kind, from: from, body: r.body);
    } catch (_) {
      return GroupEntry(gid: g.id, seq: seq, ts: ts, epoch: epoch, kind: GroupKind.system, from: from, body: const {'op': 'bad'});
    }
  }

  /// Новая запись цитирует ту, что уже надгробие (или станет им в этой же пачке): выдержка заменяется меткой, исходный текст не хранится.
  Future<GroupEntry> _quoteOfForgotten(GroupEntry e, List<GroupEntry> batch, Set<int> forgotten) async {
    final r = e.body['r'];
    if (e.deleted || r is! Map || r['s'] is! int || r['t'] == Tombstone.marker) return e;
    final s = r['s'] as int;
    var gone = forgotten.contains(s) || batch.any((x) => x.seq == s && isTombstoneEntry(x));
    if (!gone) {
      final row = await store.entries(e.gid, beforeSeq: s + 1, limit: 1);
      gone = row.isNotEmpty && row.last.seq == s && isTombstoneEntry(row.last);
    }
    if (!gone) return e;
    return GroupEntry(gid: e.gid, seq: e.seq, ts: e.ts, epoch: e.epoch, kind: e.kind, from: e.from, body: {...e.body, 'r': {...r, 't': Tombstone.marker}});
  }

  /// Забирает новые записи. Останавливается на записи, для которой ещё нет ключа (придёт позже), чтобы ничего не потерять.
  Future<int> _pull(GroupInfo g) async {
    var got = 0;
    var incomingActivity = false;
    var after = await store.lastSeq(g.id);
    if (g.firstSeq > after + 1) after = g.firstSeq - 1;   // старое на узле уже стёрто
    while (true) {
      final p = await api.pull(g.id, after, limit: 200);
      final list = [for (final x in (p['entries'] as List? ?? const [])) (x as Map).cast<String, dynamic>()];
      if (list.isEmpty) break;
      final batch = <GroupEntry>[];
      final forgotten = <int>{};   // записи с пометкой «удалено вместе с пользователем» (тело пустое): станут надгробиями
      var blocked = false;
      int? firstEpoch;
      for (final e in list) {
        if (e['seq'] is! int) { blocked = true; break; }   // без номера курсор не сдвинуть: остаток этой группы — при следующем sync
        final badEpoch = e['epoch'] != null && e['epoch'] is! int;
        final epoch = badEpoch ? 0 : (e['epoch'] ?? 0) as int;
        if (e['gone'] == true) forgotten.add(e['seq'] as int);
        final isDeleted = badEpoch || e['deleted'] == true || e['gone'] == true || (e['payload_b64'] ?? '') == '';
        final key = isDeleted ? null : await _groupKey(g.id, epoch);
        if (!isDeleted && key == null) {
          // поколение старше всех, что хранит узел, — его конверт вытеснен навсегда: запись помечаем недоступной и читаем дальше
          firstEpoch ??= await api.keys(g.id).then((k) => (k['first_epoch'] as int?) ?? 0, onError: (_) => 0);
          if (epoch < firstEpoch!) {
            batch.add(GroupEntry(gid: g.id, seq: e['seq'] as int, ts: (e['ts'] ?? 0) as int, epoch: epoch, kind: GroupKind.system, from: (e['from'] ?? '') as String, body: const {'op': 'lost'}));
            after = e['seq'] as int;
            continue;
          }
          blocked = true;
          break;
        }
        var entry = await _decode(g, e, key);
        final id = entry.body['i'];
        if (id is String && (batch.any((x) => x.body['i'] == id) || await store.hasEntryId(g.id, id))) {
          // повтор уже полученной записи (тот же шифртекст под новым номером) — не показываем и не обрабатываем
          entry = GroupEntry(gid: g.id, seq: entry.seq, ts: entry.ts, epoch: entry.epoch, kind: GroupKind.system, from: entry.from, body: const {'op': 'dup'});
        }
        entry = await _quoteOfForgotten(entry, batch, forgotten);
        batch.add(entry);
        after = e['seq'] as int;
      }
      await store.putEntries(batch);
      if (batch.any((e) => e.from != g.myAlias && !e.deleted && !isPlaceholder(e) &&
          e.body['op'] != 'dup' && !isTombstoneEntry(e))) incomingActivity = true;
      if (forgotten.isNotEmpty) await tombstoneEntries(store, g.id, forgotten);
      got += batch.length;
      if (blocked || list.length < 200) break;
    }
    if (!_retired && _soundBaseline.contains(g.id) && incomingActivity) onActivity?.call(g.id);
    _soundBaseline.add(g.id);
    return got;
  }

  /// Правила и имена берутся из служебных записей. Правила принимаются только от владельца группы.
  Future<void> _rebuildMeta(GroupInfo g) async {
    final sys = await store.entries(g.id, kind: GroupKind.system, limit: 2000);
    final names = <String, String>{};
    GroupRules? rules;
    // правила группы принимаются от владельца и админов (по их псевдонимам в текущем составе)
    final managers = <String>{if (g.canManage) g.myAlias};
    try {
      final m = await api.members(g.id);
      final roles = <String, String>{};
      final nicks = <String, String>{};
      for (final x in (m['members'] as List? ?? const [])) {
        final r = (x as Map)['role'];
        if (x['alias'] is String && x['nick'] is String && (x['nick'] as String).isNotEmpty) nicks[x['alias'] as String] = x['nick'] as String;
        if (x['alias'] is String) {
          final key = '${g.id}/${x['alias']}';
          final h = x['avatar'];
          if (h is String && h.isNotEmpty) {
            _avatarWant[key] = h;
          } else {
            _avatarWant.remove(key);
            _avatars.remove(key);
            _avatarHash.remove(key);
          }
        }
        if (x['alias'] is String && r is String) roles[x['alias'] as String] = r;
        if (r == 'owner' || r == 'admin') managers.add(x['alias'] as String);
      }
      _roles[g.id] = roles;
      _nicks[g.id] = nicks;
      try { await _recognise(g.id, m['members'] as List? ?? const []); } catch (_) {}
    } catch (_) {}
    for (final e in sys) {
      if (e.deleted) continue;
      final op = e.body['op'];
      if (op == 'name' && e.body['n'] is String) names[e.from] = (e.body['n'] as String).trim();
      if (op == 'rules' && managers.contains(e.from)) rules = GroupRules.fromBody(e.body);
    }
    _names[g.id] = names;
    if (rules != null) _rules[g.id] = rules;
    try { await _rebuildVotes(g); } catch (_) {}
  }

  // ── общий обход ──────────────────────────────────────────────────────────

  /// Список групп + ключи + новые записи. Безопасно вызывать по намёку из сокета и по таймеру; параллельные вызовы схлопываются.
  Future<void> sync({String? only}) async {
    if (_syncing || _retired) return;
    _syncing = true;
    try {
      final raw = await api.list();
      // снятый сервис делит ApiService с новым: ответ мог прийти уже от другого узла — ничего из него не принимаем
      if (_retired) return;
      final list = GroupInfo.listOf(raw);
      groups = list;
      await _trackGone(list);
      var relist = false;
      for (final g in list) {
        if (_retired) return;
        if (only != null && g.id != only) continue;
        try {
          await _publishPub(g.id);
          try { await _ensureCard(g.id); } catch (_) {}
          try { await _ensureProof(g.id); } catch (_) {}
          if (g.isOwner) {
            try { await _collectKeys(g, asOwner: true); } catch (_) {}
            await _provision(g);
            relist = true;   // номер поколения мог вырасти: список должен показывать актуальный, иначе отправка решит, что ключа нет
          } else {
            await _collectKeys(g);
          }
          final n = await _pull(g);
          var del = false;
          try { del = await _syncDeletions(g); } catch (_) {}
          if (n > 0 || del || !_rules.containsKey(g.id)) await _rebuildMeta(g);
          if (n > 0 || del) onChanged?.call(g.id);
        } on GroupApiError {
          // одна группа недоступна (например, меня выгнали) — остальные продолжают
        } catch (_) {
          // кривой ответ узла по одной группе (неверные типы полей и т. п.) тоже не останавливает остальные
        }
      }
      if (relist && !_retired) {
        final again = await api.list();
        if (!_retired) groups = GroupInfo.listOf(again);
      }
    } finally {
      _syncing = false;
    }
  }

  /// Запоминает, какие группы я видел и как они назывались; те, что пропали из списка узла, попадают в `gone`.
  Future<void> _trackGone(List<GroupInfo> list) async {
    final known = <String, String>{};
    try {
      final raw = await store.read(_knownKey);
      if (raw != null) { (jsonDecode(raw) as Map).forEach((k, v) => known['$k'] = '$v'); }
    } catch (_) {}
    if (_retired) return;
    final listed = {for (final g in list) g.id: g.name};
    var changed = false;
    listed.forEach((id, name) { if (known[id] != name) { known[id] = name; changed = true; } });
    if (changed) await store.write(_knownKey, jsonEncode(known));
    gone = [for (final e in known.entries) if (!listed.containsKey(e.key)) (e.key, e.value)];
    // Пропавшая с узла группа НЕ стирается сразу: узел могли потерять и восстановить из копии (группа тогда вернётся в список), а
    // единственная оставшаяся копия ключей и ленты — на телефоне. В списке она остаётся заглушкой «пропала на узле»; стирается сама
    // только через [goneGrace] непрерывного отсутствия или по нажатию человека.
    final missing = await _loadMissing();
    var mChanged = false;
    final nowMs = clock().millisecondsSinceEpoch;
    for (final id in List.of(missing.keys)) {
      if (listed.containsKey(id)) { missing.remove(id); mChanged = true; }   // вернулась (например, узел восстановлен из копии)
    }
    for (final g in List.of(gone)) {
      if (_retired) return;
      final since = missing.putIfAbsent(g.$1, () { mChanged = true; return nowMs; });
      if (nowMs - since > goneGrace.inMilliseconds) {
        try { await forgetLocal(g.$1); } catch (_) {}
        missing.remove(g.$1);
        mChanged = true;
      }
    }
    if (mChanged && !_retired) await store.write(_missingKey, jsonEncode(missing));
  }

  /// Через сколько непрерывного отсутствия на узле группа стирается с телефона сама.
  static const goneGrace = Duration(days: 30);
  /// Часы (в проверках подменяются).
  DateTime Function() clock = DateTime.now;
  String get _missingKey => nodeKey.isEmpty ? 'grp_missing' : 'grp_missing_$nodeKey';

  Future<Map<String, int>> _loadMissing() async {
    try {
      final raw = await store.read(_missingKey);
      if (raw != null) return {for (final e in (jsonDecode(raw) as Map).entries) '${e.key}': (e.value as num).toInt()};
    } catch (_) {}
    return {};
  }

  /// Убрать с телефона всё о группе, которой на узле уже нет.
  Future<void> forgetLocal(String gid) async {
    await store.forgetGroup(gid);
    try {
      final raw = await store.read(_knownKey);
      if (raw != null) {
        final m = (jsonDecode(raw) as Map)..remove(gid);
        await store.write(_knownKey, jsonEncode(m));
      }
    } catch (_) {}
    gone = [for (final g in gone) if (g.$1 != gid) g];
    try {
      final m = await _loadMissing();
      if (m.remove(gid) != null) await store.write(_missingKey, jsonEncode(m));
    } catch (_) {}
  }

  /// Намёк узла «запись №seq изменена» (например, стёрта модератором): перечитать именно её.
  Future<void> refreshEntry(String gid, int seq) async {
    final g = byId(gid);
    if (g == null) return;
    final p = await api.pull(gid, seq - 1, limit: 1);
    for (final x in (p['entries'] as List? ?? const [])) {
      final e = (x as Map).cast<String, dynamic>();
      if (e['seq'] != seq) continue;
      if (e['gone'] == true) {
        await tombstoneEntries(store, gid, {seq});
        onChanged?.call(gid);
      } else if (e['deleted'] == true || (e['payload_b64'] ?? '') == '') {
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
  Future<String> createGroup(String name, String topic, String about, String kind, {String? logoB64}) async {
    final gid = await api.create(name.trim(), topic.trim(), about.trim(), kind, logoB64: logoB64);
    groups = GroupInfo.listOf(await api.list());
    await sync(only: gid);   // сразу создаёт первое поколение ключа
    groups = GroupInfo.listOf(await api.list());
    return gid;
  }

  /// Одноразовый код приглашения уже использован (мной) или признан узлом недействительным: карточка в чате больше не срабатывает.
  /// Для открытой группы (кода нет) карточка гаснет только если узел ответил, что группы нет или вход запрещён (`gone`): после выхода из
  /// открытой группы в неё можно вступить снова.
  /// Отметка хранится вне записей группы (`inv_spent_…`, не `grp_…`): выход из группы стирает её ключи и ленту, но использованный
  /// одноразовый код так и остаётся использованным.
  Future<bool> inviteSpent(String gid, String code) async {
    final tail = '${gid}_${code.isEmpty ? 'open' : code}';
    return await store.read('inv_spent_$tail') == '1' || await store.read('grp_invspent_$tail') == '1';
  }

  Future<void> markInviteSpent(String gid, String code, {bool gone = false}) async {
    if (code.isNotEmpty) {
      await store.write('inv_spent_${gid}_$code', '1');
    } else if (gone) {
      await store.write('inv_spent_${gid}_open', '1');
    }
  }

  /// Группы узла, куда можно вступить или попроситься.
  Future<List<DiscoverGroup>> discover() async => [for (final j in await api.openGroups()) DiscoverGroup.fromJson(j)];

  /// Владелец включает автоочистку неактивных участников (0 — выключить).
  Future<void> setAutoClean(String gid, int days) async {
    await api.autoClean(gid, days);
    groups = GroupInfo.listOf(await api.list());
    onChanged?.call(gid);
  }

  /// Владелец меняет название, тематику и описание группы.
  Future<void> updateSettings(String gid, {String? name, String? topic, String? about, String? logoB64}) async {
    await api.settings(gid, name: name, topic: topic, about: about, logoB64: logoB64);
    groups = GroupInfo.listOf(await api.list());
    onChanged?.call(gid);
  }

  /// Следующий ключ владельца по цепочке передач: служебная запись `handover` с ключом из `wanted`, написанная тем, чей ключ я закрепил
  /// (его member_pub по списку узла совпадает с закреплённым). Сам новый владелец и рядовой участник такую запись не подтвердят.
  /// Остаётся граница: узел, сговорившийся с участником, может выдать чужую запись за запись прежнего владельца (личных подписей нет).
  /// Автор записи есть в составе — его ключ обязан совпасть с закреплённым; автора уже нет (прежний владелец вышел после передачи) —
  /// принимается запись под тем псевдонимом, который был у закреплённого владельца [pinnedAlias]. Возвращает (новый ключ, псевдоним нового).
  static (String, String?)? handoverNext(List<GroupEntry> log, List<Map<String, dynamic>> members, String pinned, Set<String> wanted, {String? pinnedAlias}) {
    for (final e in log) {
      if (e.deleted || e.kind != GroupKind.system || e.body['op'] != 'handover') continue;
      final p = e.body['pub'];
      if (p is! String || !wanted.contains(p) || p == pinned) continue;
      final author = members.where((m) => m['alias'] == e.from);
      final ok = author.isNotEmpty ? author.first['member_pub'] == pinned : (pinnedAlias != null && pinnedAlias == e.from);
      if (ok) { final to = e.body['to']; return (p, to is String ? to : null); }
    }
    return null;
  }

  /// Владелец передаёт группу участнику: служебная запись «владение передано» (с ключом нового владельца) → узел меняет роли.
  /// Прежний владелец становится админом, новый создаст новое поколение ключа.
  Future<void> transferOwner(String gid, String alias) async {
    final g = byId(gid);
    if (g == null || !g.isOwner) throw GroupSendRefused(tr('Передать группу может только её владелец'));
    final m = await api.members(gid);
    final target = (m['members'] as List? ?? const []).cast<Map>().where((x) => x['alias'] == alias).toList();
    if (target.isEmpty) throw GroupSendRefused(tr('Участник не найден'));
    final pub = target.first['member_pub'] as String?;
    if (pub == null) throw GroupSendRefused(tr('У участника ещё нет ключа группы: он должен открыть приложение'));
    final key = await _groupKey(gid, g.epoch);
    if (key == null) throw GroupSendRefused(tr('Ключ группы не создан'));
    await _append(g, GroupKind.system, {'op': 'handover', 'to': alias, 'pub': pub}, key);
    await api.transferOwner(gid, alias);
    await sync(only: gid);
  }

  /// Попросить владельца прислать приглашение в группу «по приглашению».
  Future<void> requestInvite(String gid, String myName) => api.requestJoin(gid, myName.trim());

  /// Те, чьи просьбы я попросил больше не показывать (хранится только на этом телефоне).
  Future<Set<String>> _ignored(String gid) async {
    try {
      final raw = await store.read('grp_ignore_$gid');
      if (raw != null) return {for (final x in jsonDecode(raw) as List) '$x'};
    } catch (_) {}
    return {};
  }

  /// Просьбы вступить (владелец и модераторы). Просьбы тех, кого я попросил «больше не напоминать», тихо закрываются и не показываются.
  Future<List<JoinRequest>> joinRequests(String gid) async {
    final all = [for (final j in await api.requests(gid)) JoinRequest.fromJson(j)];
    final skip = await _ignored(gid);
    if (skip.isEmpty) return all;
    final out = <JoinRequest>[];
    for (final r in all) {
      if (skip.contains(r.peerId)) {
        try { await api.resolveRequest(gid, r.id); } catch (_) {}
      } else {
        out.add(r);
      }
    }
    return out;
  }

  /// «Больше не напоминать»: просьба закрывается, а последующие просьбы этого человека не показываются.
  Future<void> ignoreRequester(String gid, JoinRequest r) async {
    final skip = await _ignored(gid)..add(r.peerId);
    await store.write('grp_ignore_$gid', jsonEncode(skip.toList()));
    await api.resolveRequest(gid, r.id);
  }

  Future<void> closeRequest(String gid, String rid) => api.resolveRequest(gid, rid);

  /// Вступить: открытая группа или по коду приглашения. После вступления имя пишется в журнал зашифрованной записью.
  Future<void> join(String gid, {String? inviteCode, required String displayName}) async {
    final (_, pub) = await _memberKeys(gid);
    await api.join(gid, inviteCode: inviteCode, memberPub: _b64(pub));
    await store.write('grp_pubsent_$gid', '1');
    groups = GroupInfo.listOf(await api.list());
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
    if (g == null) throw GroupSendRefused(tr('Группа не найдена'));
    if (g.muted) throw GroupSendRefused(tr('Вы заглушены в этой группе'));
    if (kind != GroupKind.system && !rules(gid).allows(kind)) throw GroupSendRefused(tr('В этой группе запрещён такой вид сообщений'));
    final key = await _groupKey(gid, g.epoch);
    if (key == null) throw GroupSendRefused(tr('Ключ группы ещё не получен — подождите владельца'));
    return _append(g, kind, body, key);
  }

  /// Владелец: запретить виды записей в группе (служебная запись, видна только участникам).
  Future<void> setRules(String gid, GroupRules r) async {
    final g = byId(gid);
    if (g == null || !g.canManage) throw GroupSendRefused(tr('Правила задают владелец и админ группы'));
    final key = await _groupKey(gid, g.epoch);
    if (key == null) throw GroupSendRefused(tr('Ключ группы не создан'));
    await _append(g, GroupKind.system, r.toBody(), key);
    _rules[gid] = r;
    onChanged?.call(gid);
  }

  /// Запись видна читателю? Служебные и стёртые не показываются как сообщения; запрещённые правилами — скрыты.
  bool visible(String gid, GroupEntry e) =>
      !e.deleted && e.kind != GroupKind.reaction && (e.kind != GroupKind.system || isPlaceholder(e)) && rules(gid).allows(e.kind);

  /// Монетки одобрения: группа → номер записи → баланс. Один голос на человека (последний), свои записи себе не одобряют.
  final Map<String, Map<int, int>> _coins = {};
  final Map<String, Map<int, int>> _myVotes = {};

  /// Баланс монеток под записью (0 — голосов нет или они уравновешены).
  int coins(String gid, int seq) => _coins[gid]?[seq] ?? 0;

  /// Сколько людей голосовало за запись (чтобы показывать «0», когда голоса уравновесились).
  bool hasVotes(String gid, int seq) => _voters[gid]?.contains(seq) ?? false;
  final Map<String, Set<int>> _voters = {};

  /// Мой голос за запись: 1, -1 или 0.
  int myVote(String gid, int seq) => _myVotes[gid]?[seq] ?? 0;

  /// Проголосовать за запись: 1 — одобряю, -1 — не одобряю; повтор того же голоса снимает его.
  Future<void> vote(String gid, int seq, int v) async {
    final g = byId(gid);
    if (g == null) throw GroupSendRefused(tr('Группа не найдена'));
    final next = myVote(gid, seq) == v ? 0 : v;
    await send(gid, GroupKind.reaction, {'s': seq, 'v': next});
    final mine = _myVotes.putIfAbsent(gid, () => {});
    final before = mine[seq] ?? 0;
    mine[seq] = next;
    final c = _coins.putIfAbsent(gid, () => {});
    c[seq] = (c[seq] ?? 0) - before + next;
    _voters.putIfAbsent(gid, () => {}).add(seq);
    onChanged?.call(gid);
  }

  Future<void> _rebuildVotes(GroupInfo g) async {
    final all = await store.entries(g.id, limit: 20000);
    final author = <int, String>{};
    for (final e in all) {
      if (e.kind != GroupKind.reaction && !isTombstoneEntry(e)) author[e.seq] = e.from;   // за надгробие не голосуют
    }
    final latest = <int, Map<String, int>>{};   // запись → голосующий → голос
    for (final e in all) {
      if (e.kind != GroupKind.reaction || e.deleted) continue;
      final t = e.body['s'], v = e.body['v'];
      if (t is! int || v is! int || v < -1 || v > 1) continue;
      final a = author[t];
      if (a == null || a == e.from) continue;   // нет такой записи или голос за свою
      (latest[t] ??= {})[e.from] = v;           // записи идут по порядку: остаётся последний голос
    }
    final coins = <int, int>{}, mine = <int, int>{}, voters = <int>{};
    latest.forEach((t, m) {
      coins[t] = m.values.fold(0, (a, b) => a + b);
      if (m.isNotEmpty) voters.add(t);
      final my = m[g.myAlias];
      if (my != null) mine[t] = my;
    });
    _coins[g.id] = coins;
    _myVotes[g.id] = mine;
    _voters[g.id] = voters;
  }

  /// Повреждённая (`bad`) или недоступная (`lost`, ключ поколения вытеснен с узла) запись: показывается в ленте пометкой, а не исчезает молча.
  static bool isPlaceholder(GroupEntry e) => e.kind == GroupKind.system && (e.body['op'] == 'bad' || e.body['op'] == 'lost');

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
