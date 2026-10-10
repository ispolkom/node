import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../models/group_invite.dart';
import '../services/app_state.dart';
import '../services/group_service.dart';
import '../theme.dart';

const _kindLabels = <int, String>{
  GroupKind.image: 'Картинки',
  GroupKind.file: 'Файлы',
  GroupKind.audio: 'Аудио',
  GroupKind.video: 'Видео',
  GroupKind.link: 'Ссылки',
};

/// О группе: участники и модерация, правила содержимого (владелец), приглашения, выход.
class GroupInfoScreen extends StatefulWidget {
  final String gid;
  const GroupInfoScreen({super.key, required this.gid});
  @override
  State<GroupInfoScreen> createState() => _GroupInfoScreenState();
}

class _GroupInfoScreenState extends State<GroupInfoScreen> {
  List<GroupMember> _members = const [];
  List<Map<String, dynamic>> _modlog = const [];
  List<JoinRequest> _requests = const [];
  String? _error;

  GroupService get _svc => context.read<AppState>().groups!;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  Future<void> _load() async {
    try {
      final m = await _svc.api.members(widget.gid);
      final log = await _svc.api.modlog(widget.gid);
      List<JoinRequest> reqs = const [];
      final gi = _svc.byId(widget.gid);
      if (gi != null && gi.canModerate && gi.kind == 'invite') {
        try { reqs = await _svc.joinRequests(widget.gid); } catch (_) {}
      }
      if (!mounted) return;
      setState(() {
        _members = [for (final x in (m['members'] as List? ?? const [])) GroupMember.fromJson((x as Map).cast<String, dynamic>())];
        _modlog = log.reversed.take(30).toList();
        _requests = reqs;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  Future<void> _act(String action, String alias) async {
    try {
      await _svc.moderate(widget.gid, action, alias: alias);
      await _svc.sync(only: widget.gid);
      await _load();
    } catch (e) {
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
    }
  }

  void _memberMenu(GroupInfo g, GroupMember m) {
    if (!g.canModerate || m.role == 'owner' || m.alias == g.myAlias) return;
    if (m.role == 'moderator' && !g.isOwner) return;
    final items = <(String, String, String)>[
      m.muted ? ('unmute', 'Снять заглушку', '') : ('mute', 'Заглушить', ''),
      ('kick', 'Выгнать', ''),
      ('ban', 'Запретить вход', ''),
      if (g.isOwner) (m.role == 'moderator' ? 'demote' : 'promote', m.role == 'moderator' ? 'Снять с модераторов' : 'Назначить модератором', ''),
    ];
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          for (final (a, label, _) in items)
            ListTile(title: Text(label, style: TextStyle(color: a == 'ban' || a == 'kick' ? AppTheme.incoming : AppTheme.text)), onTap: () { Navigator.pop(ctx); _act(a, m.alias); }),
        ]),
      ),
    );
  }

  /// Выбор контактов → один код на всех (по числу выбранных) → каждому уходит зашифрованное сообщение с приглашением.
  Future<void> _inviteContacts(GroupInfo g) async {
    final state = context.read<AppState>();
    final people = state.contacts.where((c) => c.saved && !state.isBlocked(c.peerId)).toList();
    if (people.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Нет контактов для приглашения')));
      return;
    }
    final chosen = <String>{};
    final ok = await showModalBottomSheet<bool>(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          const Padding(padding: EdgeInsets.all(16), child: Text('Кого пригласить', style: TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold))),
          Flexible(
            child: ListView(shrinkWrap: true, children: [
              for (final c in people)
                CheckboxListTile(
                  value: chosen.contains(c.peerId),
                  activeColor: AppTheme.accent,
                  title: Text(c.displayName, style: const TextStyle(color: AppTheme.text)),
                  onChanged: (v) => setS(() => v == true ? chosen.add(c.peerId) : chosen.remove(c.peerId)),
                ),
            ]),
          ),
          Padding(
            padding: const EdgeInsets.all(16),
            child: SizedBox(
              width: double.infinity,
              child: FilledButton(
                style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                onPressed: chosen.isEmpty ? null : () => Navigator.pop(ctx, true),
                child: Text('Отправить приглашение (${chosen.length})'),
              ),
            ),
          ),
        ]),
      )),
    );
    if (ok != true || !mounted) return;
    try {
      // отдельный код каждому: так видно, кто вошёл, и один утёкший код не пускает остальных
      var sent = 0;
      for (final id in chosen) {
        // открытая группа: код не нужен, карточка зовёт вступить сразу; «по приглашению»: свой одноразовый код каждому
        final code = g.kind == 'open' ? '' : await _svc.newInvite(g.id, uses: 1, ttlHours: 24);
        await state.sendMessage(id, GroupInviteMsg.encode(g.id, code, g.name));
        sent++;
      }
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Приглашений отправлено: $sent')));
    } catch (e) {
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
    }
  }

  /// Принять просьбу: один код → личное сообщение с приглашением тому, кто просил → просьба закрыта.
  Future<void> _answerRequest(GroupInfo g, JoinRequest r, bool accept) async {
    final state = context.read<AppState>();
    final messenger = ScaffoldMessenger.of(context);
    try {
      if (accept) {
        final code = await _svc.newInvite(g.id, uses: 1, ttlHours: 72);
        await state.sendMessage(r.peerId, GroupInviteMsg.encode(g.id, code, g.name));
      }
      await _svc.closeRequest(g.id, r.id);
      messenger.showSnackBar(SnackBar(content: Text(accept ? 'Приглашение отправлено' : 'Просьба отклонена')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('$e')));
    }
    await _load();
  }

  Future<void> _invite(GroupInfo g) async {
    try {
      final code = await _svc.newInvite(g.id);
      final text = GroupInvite(g.id, code).encode();
      if (!mounted) return;
      showDialog(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: AppTheme.surface,
          title: const Text('Приглашение', style: TextStyle(color: AppTheme.text)),
          content: SelectableText('$text\n\nДействует сутки, один раз. Передайте его лично или по защищённому каналу.', style: const TextStyle(color: AppTheme.text)),
          actions: [
            TextButton(onPressed: () { Clipboard.setData(ClipboardData(text: text)); Navigator.pop(ctx); }, child: const Text('Скопировать')),
            TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Закрыть')),
          ],
        ),
      );
    } catch (e) {
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final g = state.groups?.byId(widget.gid);
    if (g == null) return Scaffold(appBar: AppBar(title: const Text('Группа')), body: const SizedBox());
    final rules = state.groups!.rules(g.id);
    return Scaffold(
      appBar: AppBar(title: Text(g.name)),
      body: ListView(children: [
        if (g.topic.isNotEmpty) Padding(padding: const EdgeInsets.all(16), child: Text(g.topic, style: const TextStyle(color: AppTheme.textSecondary))),
        ListTile(
          leading: const Icon(Icons.badge_outlined, color: AppTheme.textSecondary),
          title: Text('Ваш псевдоним: ${g.myAlias}', style: const TextStyle(color: AppTheme.text)),
          subtitle: const Text('Свой в каждой группе; узел не знает, кто вы', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
        ),
        if (g.canModerate && g.kind != 'closed')
          ListTile(leading: const Icon(Icons.person_add_alt, color: AppTheme.accent), title: const Text('Пригласить из контактов', style: TextStyle(color: AppTheme.text)), onTap: () => _inviteContacts(g)),
        if (g.canModerate && g.kind == 'invite')
          ListTile(leading: const Icon(Icons.link, color: AppTheme.textSecondary), title: const Text('Код приглашения', style: TextStyle(color: AppTheme.text)), subtitle: const Text('Передать самому', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)), onTap: () => _invite(g)),
        if (_requests.isNotEmpty) ...[
          Padding(padding: const EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Просятся в группу (${_requests.length})', style: const TextStyle(color: AppTheme.incoming, fontWeight: FontWeight.bold))),
          for (final r in _requests)
            ListTile(
              dense: true,
              leading: const Icon(Icons.person_outline, color: AppTheme.incoming),
              title: Text(r.name.isEmpty ? 'Без имени' : r.name, style: const TextStyle(color: AppTheme.text)),
              subtitle: const Text('просит прислать приглашение', style: TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
              trailing: Row(mainAxisSize: MainAxisSize.min, children: [
                IconButton(tooltip: 'Отклонить', icon: const Icon(Icons.close, color: AppTheme.textSecondary), onPressed: () => _answerRequest(g, r, false)),
                IconButton(tooltip: 'Отправить приглашение', icon: const Icon(Icons.check, color: AppTheme.accent), onPressed: () => _answerRequest(g, r, true)),
              ]),
            ),
        ],
        if (g.isOwner) ...[
          const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Запретить в группе', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 16),
            child: Text('Содержимое зашифровано, узел его не видит: запрет держат приложения участников (не отправляют и не показывают).', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
          for (final e in _kindLabels.entries)
            SwitchListTile(
              title: Text(e.value, style: const TextStyle(color: AppTheme.text)),
              value: !rules.allows(e.key),
              onChanged: (v) async {
                final deny = {...rules.deny};
                v ? deny.add(e.key) : deny.remove(e.key);
                try { await _svc.setRules(g.id, GroupRules(deny)); } catch (err) { if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$err'))); }
                if (mounted) setState(() {});
              },
            ),
        ] else if (rules.deny.isNotEmpty)
          Padding(
            padding: const EdgeInsets.all(16),
            child: Text('Запрещено в группе: ${rules.deny.map((k) => _kindLabels[k] ?? '').where((s) => s.isNotEmpty).join(', ')}', style: const TextStyle(color: AppTheme.textSecondary)),
          ),
        const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Участники', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
        if (_error != null) Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: const TextStyle(color: AppTheme.incoming))),
        for (final m in _members)
          ListTile(
            dense: true,
            leading: Icon(m.role == 'owner' ? Icons.star : m.role == 'moderator' ? Icons.shield_outlined : Icons.person_outline, color: m.role == 'member' ? AppTheme.textSecondary : AppTheme.accent),
            title: Text(state.groups!.displayName(g.id, m.alias) + (m.alias == g.myAlias ? ' (вы)' : ''), style: const TextStyle(color: AppTheme.text)),
            subtitle: Text('${m.alias}${m.muted ? ' · заглушён' : ''}${m.hasKey || m.role == 'owner' ? '' : ' · ждёт ключ'}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
            onLongPress: () => _memberMenu(g, m),
            trailing: g.canModerate && m.role != 'owner' && m.alias != g.myAlias && !(m.role == 'moderator' && !g.isOwner)
                ? IconButton(tooltip: 'Действия', icon: const Icon(Icons.more_vert, color: AppTheme.textSecondary), onPressed: () => _memberMenu(g, m))
                : null,
          ),
        if (_modlog.isNotEmpty) ...[
          const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Действия модераторов', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
          for (final l in _modlog)
            ListTile(dense: true, title: Text('${l['by'] ?? ''}: ${l['action'] ?? ''}${l['target'] != null ? ' → ${l['target']}' : ''}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
        ],
        if (!g.isOwner)
          ListTile(
            leading: const Icon(Icons.logout, color: AppTheme.incoming),
            title: const Text('Выйти из группы', style: TextStyle(color: AppTheme.incoming)),
            onTap: () async {
              final nav = Navigator.of(context);
              try { await _svc.leave(g.id); } catch (_) {}
              nav.popUntil((r) => r.isFirst);
            },
          )
        else
          const Padding(padding: EdgeInsets.all(16), child: Text('Группу удаляют только на странице узла, по паролю.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
      ]),
    );
  }
}
