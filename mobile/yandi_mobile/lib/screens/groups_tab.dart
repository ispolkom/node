import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/group.dart';
import '../models/group_invite.dart';
import '../services/app_state.dart';
import '../theme.dart';
import 'group_chat_screen.dart';

/// Вкладка «Группы»: мои группы, открытые группы узла, вступление по приглашению.
class GroupsTab extends StatefulWidget {
  const GroupsTab({super.key});
  @override
  State<GroupsTab> createState() => _GroupsTabState();
}

class _GroupsTabState extends State<GroupsTab> {
  bool _loaded = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
  }

  Future<void> _refresh() async {
    await context.read<AppState>().syncGroups();
    if (mounted) setState(() => _loaded = true);
  }

  String _roleLabel(String r) => switch (r) { 'owner' => 'владелец', 'moderator' => 'модератор', _ => 'участник' };

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final list = state.groups?.groups ?? const <GroupInfo>[];
    return Scaffold(
      backgroundColor: Colors.transparent,
      floatingActionButton: FloatingActionButton.extended(
        backgroundColor: AppTheme.accent,
        foregroundColor: Colors.black,
        onPressed: () => _joinSheet(context),
        icon: const Icon(Icons.login),
        label: const Text('Вступить'),
      ),
      body: RefreshIndicator(
        onRefresh: _refresh,
        child: list.isEmpty
            ? ListView(children: [
                SizedBox(
                  height: 420,
                  child: Center(
                    child: Padding(
                      padding: const EdgeInsets.all(32),
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                        const Icon(Icons.groups_outlined, color: AppTheme.textSecondary, size: 64),
                        const SizedBox(height: 16),
                        Text(_loaded ? 'Групп пока нет' : 'Загрузка…', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 15)),
                        const SizedBox(height: 8),
                        const Text('Группы создаются на странице узла (ПК). Вступить можно по приглашению или в открытую группу узла.',
                            textAlign: TextAlign.center, style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      ]),
                    ),
                  ),
                ),
              ])
            : ListView.separated(
                itemCount: list.length,
                separatorBuilder: (_, __) => const Divider(height: 1),
                itemBuilder: (_, i) {
                  final g = list[i];
                  return ListTile(
                    leading: CircleAvatar(
                      backgroundColor: AppTheme.surface,
                      child: Text(g.name.isEmpty ? '?' : g.name.characters.first.toUpperCase(), style: const TextStyle(color: AppTheme.accent)),
                    ),
                    title: Text(g.name, style: const TextStyle(color: AppTheme.text)),
                    subtitle: Text('${g.topic.isEmpty ? '' : '${g.topic} · '}${g.members} уч. · вы: ${_roleLabel(g.role)}${g.muted ? ' · заглушены' : ''}',
                        style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                    trailing: state.groups!.hasProblem(g.id) ? const Icon(Icons.warning_amber, color: AppTheme.incoming) : null,
                    onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupChatScreen(gid: g.id))),
                  );
                },
              ),
      ),
    );
  }

  void _joinSheet(BuildContext context) {
    final state = context.read<AppState>();
    final codeCtrl = TextEditingController();
    final nameCtrl = TextEditingController();
    String? error;
    bool busy = false;
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) {
        Future<void> doJoin(String gid, String? code) async {
          final name = nameCtrl.text.trim();
          if (name.isEmpty) { setS(() => error = 'Введите имя, которое увидят в группе'); return; }
          setS(() { busy = true; error = null; });
          try {
            await state.groups!.join(gid, inviteCode: code, displayName: name);
            await state.syncGroups();
            if (ctx.mounted) Navigator.pop(ctx);
          } catch (e) {
            setS(() { busy = false; error = '$e'; });
          }
        }

        return Padding(
          padding: EdgeInsets.fromLTRB(16, 16, 16, 16 + MediaQuery.of(ctx).viewInsets.bottom),
          child: SingleChildScrollView(
            child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
              const Text('Вступить в группу', style: TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold)),
              const SizedBox(height: 12),
              TextField(controller: nameCtrl, style: const TextStyle(color: AppTheme.text),
                  decoration: const InputDecoration(labelText: 'Ваше имя в группе', helperText: 'Узел его не видит: оно пишется зашифрованно')),
              const SizedBox(height: 8),
              TextField(controller: codeCtrl, style: const TextStyle(color: AppTheme.text),
                  decoration: const InputDecoration(labelText: 'Приглашение', hintText: 'YANDI-GROUP-1:…')),
              const SizedBox(height: 8),
              if (error != null) Padding(padding: const EdgeInsets.only(bottom: 8), child: Text(error!, style: const TextStyle(color: AppTheme.incoming))),
              FilledButton(
                style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                onPressed: busy ? null : () {
                  final inv = GroupInvite.tryParse(codeCtrl.text);
                  if (inv == null) { setS(() => error = 'Неверное приглашение'); return; }
                  doJoin(inv.gid, inv.code);
                },
                child: const Text('Вступить по приглашению'),
              ),
              const SizedBox(height: 16),
              const Text('Открытые группы этого узла', style: TextStyle(color: AppTheme.textSecondary)),
              FutureBuilder<List<Map<String, dynamic>>>(
                future: state.groups!.api.openGroups(),
                builder: (_, snap) {
                  final open = snap.data ?? const [];
                  if (open.isEmpty) return const Padding(padding: EdgeInsets.all(8), child: Text('нет', style: TextStyle(color: AppTheme.textSecondary)));
                  return Column(children: [
                    for (final g in open)
                      ListTile(
                        dense: true,
                        title: Text('${g['name']}', style: const TextStyle(color: AppTheme.text)),
                        subtitle: Text('${g['topic'] ?? ''} · ${g['members'] ?? 0} уч.', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                        trailing: TextButton(onPressed: busy ? null : () => doJoin(g['id'] as String, null), child: const Text('Вступить')),
                      ),
                  ]);
                },
              ),
            ]),
          ),
        );
      }),
    );
  }
}
