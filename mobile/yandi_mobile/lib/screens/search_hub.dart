import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/contact.dart';
import '../models/group.dart';
import '../services/app_state.dart';
import '../theme.dart';
import '../widgets/avatar_circle.dart';
import '../widgets/secure_field.dart';
import 'home_screen.dart' show showSaveContactDialog;

/// Видимые пользователи: сопряжённые с узлом и не удалённые им за бездействие (давно не бывшие в сети не показываются).
List<Contact> visibleUsers(AppState state, {bool withHidden = false}) {
  final now = DateTime.now().millisecondsSinceEpoch;
  final list = [
    for (final c in state.contacts)
      if (!c.isStale(now) && (withHidden || !state.hiddenUsers.contains(c.peerId))) c,
  ];
  list.sort((a, b) {
    if (a.online != b.online) return a.online ? -1 : 1;
    return a.displayName.toLowerCase().compareTo(b.displayName.toLowerCase());
  });
  return list;
}

/// Две карточки страницы поиска: «Пользователи» и «Группы», в каждой — количество.
class SearchHub extends StatefulWidget {
  const SearchHub({super.key});
  @override
  State<SearchHub> createState() => _SearchHubState();
}

class _SearchHubState extends State<SearchHub> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) context.read<AppState>().refreshDirectory();
    });
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final users = visibleUsers(state).length;
    return ListView(
      padding: const EdgeInsets.all(12),
      children: [
        _HubCard(
          icon: Icons.people_alt_outlined,
          title: 'Пользователи',
          count: '$users',
          subtitle: 'Сопряжённые с узлом: ники, аватарки, статус',
          onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const UsersScreen())),
        ),
        const SizedBox(height: 10),
        FutureBuilder<List<DiscoverGroup>>(
          future: state.groups?.discover() ?? Future.value(const <DiscoverGroup>[]),
          builder: (_, snap) {
            final n = [for (final g in snap.data ?? const <DiscoverGroup>[]) if (!state.hiddenGroups.contains(g.id)) g].length;
            return _HubCard(
              icon: Icons.groups_2_outlined,
              title: 'Группы',
              count: snap.connectionState == ConnectionState.done ? '$n' : '…',
              subtitle: 'Все группы этого узла: вступить или попросить приглашение',
              onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const GroupsCatalogScreen())),
            );
          },
        ),
      ],
    );
  }
}

class _HubCard extends StatelessWidget {
  final IconData icon;
  final String title, count, subtitle;
  final VoidCallback onTap;
  const _HubCard({required this.icon, required this.title, required this.count, required this.subtitle, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Material(
      color: AppTheme.surface,
      borderRadius: BorderRadius.circular(14),
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Row(children: [
            Icon(icon, color: AppTheme.accent, size: 34),
            const SizedBox(width: 14),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(title, style: const TextStyle(color: AppTheme.text, fontSize: 17, fontWeight: FontWeight.w700)),
                const SizedBox(height: 2),
                Text(subtitle, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              ]),
            ),
            Text(count, style: const TextStyle(color: AppTheme.accent, fontSize: 26, fontWeight: FontWeight.w800)),
          ]),
        ),
      ),
    );
  }
}

/// Кружок и слово статуса: зелёный «Онлайн» или красный «Не в сети».
class StatusBadge extends StatelessWidget {
  final bool online;
  const StatusBadge({super.key, required this.online});

  @override
  Widget build(BuildContext context) {
    final color = online ? Colors.greenAccent : Colors.redAccent;
    return Row(mainAxisSize: MainAxisSize.min, children: [
      Container(width: 10, height: 10, decoration: BoxDecoration(shape: BoxShape.circle, color: color)),
      const SizedBox(width: 6),
      Text(online ? 'Онлайн' : 'Не в сети', style: TextStyle(color: color, fontSize: 12)),
    ]);
  }
}

class UsersScreen extends StatefulWidget {
  const UsersScreen({super.key});
  @override
  State<UsersScreen> createState() => _UsersScreenState();
}

class _UsersScreenState extends State<UsersScreen> {
  bool _showHidden = false;

  @override
  void initState() {
    super.initState();
    // ники и картинки устройств узла подтягиваются при открытии (в памяти, на диск не пишутся)
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final st = context.read<AppState>();
      st.refreshContacts();
      st.refreshDirectory();
    });
  }

  void _menu(BuildContext context, AppState state, Contact c) {
    final hidden = state.hiddenUsers.contains(c.peerId);
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(
            leading: AvatarCircle(peerId: c.peerId, label: c.displayName),
            title: Text(c.displayName, style: const TextStyle(color: AppTheme.text)),
            subtitle: StatusBadge(online: c.online),
          ),
          if (!c.saved)
            ListTile(
              leading: const Icon(Icons.person_add_alt_1, color: AppTheme.accent),
              title: const Text('Добавить в контакты', style: TextStyle(color: AppTheme.text)),
              onTap: () {
                Navigator.pop(ctx);
                showSaveContactDialog(context, c.peerId, c.displayName);
              },
            ),
          ListTile(
            leading: Icon(hidden ? Icons.visibility : Icons.visibility_off, color: AppTheme.textSecondary),
            title: Text(hidden ? 'Показать' : 'Скрыть', style: const TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              state.setHidden(group: false, id: c.peerId, hidden: !hidden);
            },
          ),
        ]),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final all = visibleUsers(state, withHidden: true);
    final shown = _showHidden ? all : all.where((c) => !state.hiddenUsers.contains(c.peerId)).toList();
    final hiddenCount = all.length - all.where((c) => !state.hiddenUsers.contains(c.peerId)).length;
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        iconTheme: const IconThemeData(color: AppTheme.text),
        title: Text('Пользователи (${all.length - hiddenCount})', style: const TextStyle(color: AppTheme.text)),
      ),
      body: ListView(children: [
        if (shown.isEmpty)
          const Padding(
            padding: EdgeInsets.all(32),
            child: Center(child: Text('Пока никого нет', style: TextStyle(color: AppTheme.textSecondary))),
          ),
        for (final c in shown)
          InkWell(
            onTap: () => _menu(context, state, c),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
              decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: AppTheme.divider, width: 0.5))),
              child: Row(children: [
                // аватар принадлежит тому, кто создал аккаунт: показываем как есть, не меняем
                AvatarCircle(peerId: c.peerId, label: c.displayName),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(c.displayName, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w600, fontSize: 15)),
                    const SizedBox(height: 3),
                    Row(children: [
                      StatusBadge(online: c.online),
                      if (c.saved) const Padding(
                        padding: EdgeInsets.only(left: 10),
                        child: Text('в контактах', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      ),
                      if (state.hiddenUsers.contains(c.peerId)) const Padding(
                        padding: EdgeInsets.only(left: 10),
                        child: Text('скрыт', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      ),
                    ]),
                  ]),
                ),
                const Icon(Icons.more_vert, color: AppTheme.textSecondary),
              ]),
            ),
          ),
        if (hiddenCount > 0)
          TextButton(
            onPressed: () => setState(() => _showHidden = !_showHidden),
            child: Text(_showHidden ? 'Не показывать скрытых' : 'Показать скрытых ($hiddenCount)',
                style: const TextStyle(color: AppTheme.textSecondary)),
          ),
      ]),
    );
  }
}

class GroupsCatalogScreen extends StatefulWidget {
  const GroupsCatalogScreen({super.key});
  @override
  State<GroupsCatalogScreen> createState() => _GroupsCatalogScreenState();
}

class _GroupsCatalogScreenState extends State<GroupsCatalogScreen> {
  bool _showHidden = false;
  late Future<List<DiscoverGroup>> _future;

  @override
  void initState() {
    super.initState();
    _future = _load();
  }

  Future<List<DiscoverGroup>> _load() => context.read<AppState>().groups?.discover() ?? Future.value(const <DiscoverGroup>[]);

  Future<String?> _askName(BuildContext context, String title) {
    final ctrl = TextEditingController();
    return showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(title, style: const TextStyle(color: AppTheme.text)),
        content: SingleChildScrollView(
          child: SecureField(controller: ctrl, hint: 'Ваше имя в группе', onSubmitted: () => Navigator.pop(ctx, ctrl.text.trim())),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(onPressed: () => Navigator.pop(ctx, ctrl.text.trim()), child: const Text('Продолжить', style: TextStyle(color: AppTheme.accent))),
        ],
      ),
    );
  }

  Future<void> _join(DiscoverGroup g) async {
    final state = context.read<AppState>();
    final messenger = ScaffoldMessenger.of(context);
    if (!g.ownerPresent) {
      messenger.showSnackBar(const SnackBar(content: Text('Владелец группы отсутствует: ключ вам выдать некому, вступить нельзя')));
      return;
    }
    final name = await _askName(context, g.isOpen ? 'Вступить в «${g.name}»' : 'Попросить вход в «${g.name}»');
    if (name == null || name.isEmpty || !mounted) return;
    try {
      if (g.isOpen) {
        await state.groups!.join(g.id, displayName: name);
        await state.syncGroups();
        messenger.showSnackBar(const SnackBar(content: Text('Вы в группе — она во вкладке «Группы»')));
      } else {
        await state.groups!.requestInvite(g.id, name);
        messenger.showSnackBar(const SnackBar(content: Text('Запрос отправлен владельцу и модераторам. Когда подтвердят, придёт карточка-приглашение')));
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('Не получилось: $e')));
    }
    if (mounted) setState(() => _future = _load());
  }

  void _menu(DiscoverGroup g) {
    final state = context.read<AppState>();
    final hidden = state.hiddenGroups.contains(g.id);
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(title: Text(g.name, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w700)), subtitle: Text(g.topic, style: const TextStyle(color: AppTheme.textSecondary))),
          ListTile(
            leading: Icon(g.isOpen ? Icons.login : Icons.mail_outline, color: AppTheme.accent),
            title: Text(g.requested ? 'Запрос уже отправлен' : (g.isOpen ? 'Вступить' : 'Попросить вход'), style: const TextStyle(color: AppTheme.text)),
            enabled: !g.requested,
            onTap: () {
              Navigator.pop(ctx);
              _join(g);
            },
          ),
          ListTile(
            leading: Icon(hidden ? Icons.visibility : Icons.visibility_off, color: AppTheme.textSecondary),
            title: Text(hidden ? 'Показать' : 'Скрыть', style: const TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              state.setHidden(group: true, id: g.id, hidden: !hidden);
            },
          ),
        ]),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        iconTheme: const IconThemeData(color: AppTheme.text),
        title: const Text('Группы', style: TextStyle(color: AppTheme.text)),
        actions: [IconButton(icon: const Icon(Icons.refresh), onPressed: () => setState(() => _future = _load()))],
      ),
      body: FutureBuilder<List<DiscoverGroup>>(
        future: _future,
        builder: (_, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(child: CircularProgressIndicator(color: AppTheme.accent));
          }
          if (snap.hasError) {
            return const Center(child: Text('Не удалось получить список групп узла', style: TextStyle(color: AppTheme.textSecondary)));
          }
          final all = snap.data ?? const <DiscoverGroup>[];
          final hiddenCount = all.where((g) => state.hiddenGroups.contains(g.id)).length;
          final shown = _showHidden ? all : all.where((g) => !state.hiddenGroups.contains(g.id)).toList();
          return ListView(children: [
            if (shown.isEmpty)
              const Padding(padding: EdgeInsets.all(32), child: Center(child: Text('Групп пока нет', style: TextStyle(color: AppTheme.textSecondary)))),
            for (final g in shown)
              InkWell(
                onTap: () => _menu(g),
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                  decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: AppTheme.divider, width: 0.5))),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(g.name, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w700, fontSize: 16)),
                    if (g.topic.isNotEmpty) Text(g.topic, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
                    const SizedBox(height: 4),
                    Wrap(spacing: 12, children: [
                      Row(mainAxisSize: MainAxisSize.min, children: [
                        Icon(g.isOpen ? Icons.lock_open : Icons.lock_outline, size: 14, color: AppTheme.accent),
                        const SizedBox(width: 4),
                        Text(g.isOpen ? 'Открытая' : 'По приглашению', style: const TextStyle(color: AppTheme.accent, fontSize: 12)),
                      ]),
                      Text('${g.members} уч.', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      if (!g.ownerPresent) const Text('владелец отсутствует', style: TextStyle(color: Colors.orangeAccent, fontSize: 12)),
                      if (g.requested) const Text('запрос отправлен', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      if (state.hiddenGroups.contains(g.id)) const Text('скрыта', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                    ]),
                  ]),
                ),
              ),
            if (hiddenCount > 0)
              TextButton(
                onPressed: () => setState(() => _showHidden = !_showHidden),
                child: Text(_showHidden ? 'Не показывать скрытые' : 'Показать скрытые ($hiddenCount)', style: const TextStyle(color: AppTheme.textSecondary)),
              ),
          ]);
        },
      ),
    );
  }
}
