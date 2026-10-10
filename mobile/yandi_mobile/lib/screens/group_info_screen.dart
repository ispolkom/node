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
      if (!mounted) return;
      setState(() {
        _members = [for (final x in (m['members'] as List? ?? const [])) GroupMember.fromJson((x as Map).cast<String, dynamic>())];
        _modlog = log.reversed.take(30).toList();
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
        if (g.kind == 'invite' && g.canModerate)
          ListTile(leading: const Icon(Icons.person_add_alt, color: AppTheme.accent), title: const Text('Пригласить', style: TextStyle(color: AppTheme.text)), onTap: () => _invite(g)),
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
