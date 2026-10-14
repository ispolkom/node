import 'dart:convert';

/// Группа в списке (ответ узла `GET /mobile/groups`).
class GroupInfo {
  final String id, name, topic, about, kind, myAlias, role;
  final bool muted, rekeyNeeded;
  /// Устройство владельца группы сопряжено с узлом: без него новый ключ группы никому не выдать.
  final bool ownerPresent;
  final int members, epoch, firstSeq, lastSeq, requests;
  /// Автоочистка: сколько дней без записей терпят рядового участника (0 — выключена).
  final int autoCleanDays;
  final List<String> pendingKeys;
  const GroupInfo({
    required this.id, required this.name, required this.topic, this.about = '', required this.kind, required this.myAlias, required this.role,
    required this.muted, required this.rekeyNeeded, required this.members, required this.epoch, required this.firstSeq,
    required this.lastSeq, required this.pendingKeys, this.requests = 0, this.autoCleanDays = 0, this.ownerPresent = true,
  });

  factory GroupInfo.fromJson(Map<String, dynamic> j) => GroupInfo(
        id: j['id'] as String,
        name: (j['name'] ?? '') as String,
        topic: (j['topic'] ?? '') as String,
        about: (j['about'] ?? '') as String,
        kind: (j['kind'] ?? 'invite') as String,
        myAlias: (j['my_alias'] ?? '') as String,
        role: (j['role'] ?? 'member') as String,
        muted: j['muted'] == true,
        rekeyNeeded: j['rekey_needed'] == true,
        ownerPresent: j['owner_present'] != false,
        members: (j['members'] ?? 0) as int,
        epoch: (j['epoch'] ?? 0) as int,
        firstSeq: (j['first_seq'] ?? 0) as int,
        lastSeq: (j['last_seq'] ?? 0) as int,
        requests: (j['requests'] ?? 0) as int,
        autoCleanDays: (j['auto_clean_days'] ?? 0) as int,
        pendingKeys: [for (final a in (j['pending_keys'] ?? const []) as List) a as String],
      );

  /// Список групп от узла: строка с неверными полями пропускается, остальные группы остаются (одна кривая строка не ломает всё).
  static List<GroupInfo> listOf(List raw) {
    final out = <GroupInfo>[];
    for (final j in raw) {
      try {
        out.add(GroupInfo.fromJson((j as Map).cast<String, dynamic>()));
      } catch (_) {}
    }
    return out;
  }

  bool get isOwner => role == 'owner';
  bool get isAdmin => role == 'admin';
  /// Владелец и админ: настройки группы, правила, роли «модератор/участник».
  bool get canManage => role == 'owner' || role == 'admin';
  bool get canModerate => role == 'owner' || role == 'admin' || role == 'moderator';

  /// Может ли стереть запись автора с ролью [authorRole] (null — автор ушёл или не рядовой известен как участник).
  /// Матрица узла: модератор — записи рядовых и свои; админ — рядовых, модераторов и свои; владелец — любые, кроме чужих владельца
  /// (владелец один). Рядовой участник стирать не может: он только скрывает автора у себя.
  bool canDeleteEntryOf(String? authorRole, {required bool mine}) {
    if (!canModerate) return false;
    if (mine) return true;
    return switch (authorRole) {
      null || 'member' => true,
      'moderator' => role == 'owner' || role == 'admin',
      'admin' => role == 'owner',
      _ => false,
    };
  }

  /// Может ли применить к автору [authorRole] «заглушить / выгнать / запретить вход»: тем же правилам, что и стирание.
  bool canActOnRole(String authorRole) => switch (authorRole) {
        'owner' => false,
        'admin' => role == 'owner',
        'moderator' => role == 'owner' || role == 'admin',
        _ => canModerate,
      };
}

class GroupMember {
  final String alias, role;
  final bool muted, hasKey;
  final String? memberPub;
  /// ник, который участник сам раскрыл в этой группе (null — виден только псевдоним)
  final String? nick;
  const GroupMember({required this.alias, required this.role, required this.muted, required this.hasKey, this.memberPub, this.nick});

  factory GroupMember.fromJson(Map<String, dynamic> j) => GroupMember(
        alias: j['alias'] as String,
        role: (j['role'] ?? 'member') as String,
        muted: j['muted'] == true,
        hasKey: j['has_key'] == true,
        memberPub: j['member_pub'] as String?,
        nick: j['nick'] as String?,
      );
}

/// Запись журнала группы после расшифровки.
class GroupEntry {
  final String gid;
  final int seq, ts, epoch, kind;
  final String from;
  final Map<String, dynamic> body;
  final bool deleted;
  const GroupEntry({required this.gid, required this.seq, required this.ts, required this.epoch, required this.kind, required this.from, required this.body, this.deleted = false});

  String get bodyJson => jsonEncode(body);

  GroupEntry asDeleted() => GroupEntry(gid: gid, seq: seq, ts: ts, epoch: epoch, kind: kind, from: from, body: const {}, deleted: true);
}

/// Правила содержимого группы: владелец задаёт, какие виды записей запрещены. Это правило клиентов: отправитель отказывается,
/// получатели скрывают (узел содержимого не видит и принудить не может).
class GroupRules {
  final Set<int> deny;
  const GroupRules([this.deny = const {}]);
  bool allows(int kind) => !deny.contains(kind);
  Map<String, dynamic> toBody() => {'op': 'rules', 'deny': deny.toList()..sort()};
  static GroupRules fromBody(Map<String, dynamic> b) => GroupRules({
        for (final k in (b['deny'] as List? ?? const [])) if (k is int) k,
      });
}

/// Группа узла, куда можно вступить или попроситься (из списка «Группы узла»).
class DiscoverGroup {
  final String id, name, topic, about, kind;
  final int members;
  final bool requested;
  const DiscoverGroup({required this.id, required this.name, required this.topic, this.about = '', required this.kind, required this.members, required this.requested});

  factory DiscoverGroup.fromJson(Map<String, dynamic> j) => DiscoverGroup(
        id: j['id'] as String,
        name: (j['name'] ?? '') as String,
        topic: (j['topic'] ?? '') as String,
        about: (j['about'] ?? '') as String,
        kind: (j['kind'] ?? 'open') as String,
        members: (j['members'] ?? 0) as int,
        requested: j['requested'] == true,
      );

  bool get isOpen => kind == 'open';
}

/// Просьба вступить: человек просит владельца прислать приглашение.
class JoinRequest {
  final String id, peerId, name;
  final int ts;
  const JoinRequest({required this.id, required this.peerId, required this.name, required this.ts});

  factory JoinRequest.fromJson(Map<String, dynamic> j) =>
      JoinRequest(id: j['id'] as String, peerId: j['peer_id'] as String, name: (j['name'] ?? '') as String, ts: (j['ts'] ?? 0) as int);
}
