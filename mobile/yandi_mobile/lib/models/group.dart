import 'dart:convert';

/// Группа в списке (ответ узла `GET /mobile/groups`).
class GroupInfo {
  final String id, name, topic, kind, myAlias, role;
  final bool muted, rekeyNeeded;
  final int members, epoch, firstSeq, lastSeq;
  final List<String> pendingKeys;
  const GroupInfo({
    required this.id, required this.name, required this.topic, required this.kind, required this.myAlias, required this.role,
    required this.muted, required this.rekeyNeeded, required this.members, required this.epoch, required this.firstSeq,
    required this.lastSeq, required this.pendingKeys,
  });

  factory GroupInfo.fromJson(Map<String, dynamic> j) => GroupInfo(
        id: j['id'] as String,
        name: (j['name'] ?? '') as String,
        topic: (j['topic'] ?? '') as String,
        kind: (j['kind'] ?? 'closed') as String,
        myAlias: (j['my_alias'] ?? '') as String,
        role: (j['role'] ?? 'member') as String,
        muted: j['muted'] == true,
        rekeyNeeded: j['rekey_needed'] == true,
        members: (j['members'] ?? 0) as int,
        epoch: (j['epoch'] ?? 0) as int,
        firstSeq: (j['first_seq'] ?? 0) as int,
        lastSeq: (j['last_seq'] ?? 0) as int,
        pendingKeys: [for (final a in (j['pending_keys'] ?? const []) as List) a as String],
      );

  bool get isOwner => role == 'owner';
  bool get canModerate => role == 'owner' || role == 'moderator';
}

class GroupMember {
  final String alias, role;
  final bool muted, hasKey;
  final String? memberPub;
  const GroupMember({required this.alias, required this.role, required this.muted, required this.hasKey, this.memberPub});

  factory GroupMember.fromJson(Map<String, dynamic> j) => GroupMember(
        alias: j['alias'] as String,
        role: (j['role'] ?? 'member') as String,
        muted: j['muted'] == true,
        hasKey: j['has_key'] == true,
        memberPub: j['member_pub'] as String?,
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
