import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/contact.dart';
import '../models/group.dart';
import '../services/app_state.dart';
import '../theme.dart';
import '../widgets/avatar_circle.dart';
import '../widgets/secure_field.dart';
import '../widgets/group_logo.dart';
import '../widgets/complaint_sheet.dart';
import 'home_screen.dart' show showSaveContactDialog;
import '../widgets/locked_notice.dart';
import '../l10n.dart';

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
          title: tr('Пользователи'),
          count: '$users',
          subtitle: tr('Сопряжённые с узлом: ники, аватарки, статус'),
          onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const UsersScreen())),
        ),
        const SizedBox(height: 10),
        FutureBuilder<List<DiscoverGroup>>(
          future: state.groups?.discover() ?? Future.value(const <DiscoverGroup>[]),
          builder: (_, snap) {
            final n = [for (final g in snap.data ?? const <DiscoverGroup>[]) if (!state.hiddenGroups.contains(g.id)) g].length;
            return _HubCard(
              icon: Icons.groups_2_outlined,
              title: tr('Группы'),
              count: snap.connectionState == ConnectionState.done ? '$n' : '…',
              subtitle: tr('Все группы этого узла: вступить или попросить приглашение'),
              onTap: () => state.isGuest
                  ? showLockedNotice(context, what: tr('Группы'))
                  : Navigator.of(context).push(MaterialPageRoute(builder: (_) => const GroupsCatalogScreen())),
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
      Text(online ? tr('Онлайн') : tr('Не в сети'), style: TextStyle(color: color, fontSize: 12)),
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
              title: Text(tr('Добавить в контакты'), style: TextStyle(color: AppTheme.text)),
              onTap: () {
                Navigator.pop(ctx);
                showSaveContactDialog(context, c.peerId, c.displayName);
              },
            ),
          ListTile(
            leading: Icon(hidden ? Icons.visibility : Icons.visibility_off, color: AppTheme.textSecondary),
            title: Text(hidden ? tr('Показать') : tr('Скрыть'), style: const TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              state.setHidden(group: false, id: c.peerId, hidden: !hidden);
            },
          ),
          ListTile(
            leading: const Icon(Icons.flag_outlined, color: Colors.redAccent),
            title: Text(tr('Пожаловаться'), style: TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              showComplaintSheet(context, kind: 'user', targetId: c.peerId, title: c.displayName);
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
        title: Text(trf('Пользователи ({n})', {'n': all.length - hiddenCount}), style: const TextStyle(color: AppTheme.text)),
      ),
      body: ListView(children: [
        if (shown.isEmpty)
          Padding(
            padding: EdgeInsets.all(32),
            child: Center(child: Text(tr('Пока никого нет'), style: TextStyle(color: AppTheme.textSecondary))),
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
                      if (c.saved) Padding(
                        padding: EdgeInsets.only(left: 10),
                        child: Text(tr('в контактах'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      ),
                      if (state.hiddenUsers.contains(c.peerId)) Padding(
                        padding: EdgeInsets.only(left: 10),
                        child: Text(tr('скрыт'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
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
            child: Text(_showHidden ? tr('Не показывать скрытых') : trf('Показать скрытых ({n})', {'n': hiddenCount}),
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
          child: SecureField(controller: ctrl, hint: tr('Ваше имя в группе'), onSubmitted: () => Navigator.pop(ctx, ctrl.text.trim())),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Отмена'), style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(onPressed: () => Navigator.pop(ctx, ctrl.text.trim()), child: Text(tr('Продолжить'), style: TextStyle(color: AppTheme.accent))),
        ],
      ),
    );
  }

  Future<void> _join(DiscoverGroup g) async {
    final state = context.read<AppState>();
    final messenger = ScaffoldMessenger.of(context);
    if (!g.ownerPresent) {
      messenger.showSnackBar(SnackBar(content: Text(tr('Владелец группы отсутствует: ключ вам выдать некому, вступить нельзя'))));
      return;
    }
    final name = await _askName(context, g.isOpen ? trf('Вступить в «{n}»', {'n': g.name}) : trf('Попросить вход в «{n}»', {'n': g.name}));
    if (name == null || name.isEmpty || !mounted) return;
    try {
      if (g.isOpen) {
        await state.groups!.join(g.id, displayName: name);
        await state.syncGroups();
        messenger.showSnackBar(SnackBar(content: Text(tr('Вы в группе — она во вкладке «Группы»'))));
      } else {
        await state.groups!.requestInvite(g.id, name);
        messenger.showSnackBar(SnackBar(content: Text(tr('Запрос отправлен владельцу и модераторам. Когда подтвердят, придёт карточка-приглашение'))));
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(trf('Не получилось: {e}', {'e': e}))));
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
            title: Text(g.requested ? tr('Запрос уже отправлен') : (g.isOpen ? tr('Вступить') : tr('Попросить вход')), style: const TextStyle(color: AppTheme.text)),
            enabled: !g.requested,
            onTap: () {
              Navigator.pop(ctx);
              _join(g);
            },
          ),
          ListTile(
            leading: Icon(hidden ? Icons.visibility : Icons.visibility_off, color: AppTheme.textSecondary),
            title: Text(hidden ? tr('Показать') : tr('Скрыть'), style: const TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              state.setHidden(group: true, id: g.id, hidden: !hidden);
            },
          ),
          ListTile(
            leading: const Icon(Icons.flag_outlined, color: Colors.redAccent),
            title: Text(tr('Пожаловаться'), style: TextStyle(color: AppTheme.text)),
            onTap: () {
              Navigator.pop(ctx);
              showComplaintSheet(context, kind: 'group', targetId: g.id, title: g.name);
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
        title: Text(tr('Группы'), style: TextStyle(color: AppTheme.text)),
        actions: [IconButton(icon: const Icon(Icons.refresh), onPressed: () => setState(() => _future = _load()))],
      ),
      body: FutureBuilder<List<DiscoverGroup>>(
        future: _future,
        builder: (_, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(child: CircularProgressIndicator(color: AppTheme.accent));
          }
          if (snap.hasError) {
            return Center(child: Text(tr('Не удалось получить список групп узла'), style: TextStyle(color: AppTheme.textSecondary)));
          }
          final all = snap.data ?? const <DiscoverGroup>[];
          final hiddenCount = all.where((g) => state.hiddenGroups.contains(g.id)).length;
          final shown = _showHidden ? all : all.where((g) => !state.hiddenGroups.contains(g.id)).toList();
          return ListView(children: [
            if (shown.isEmpty)
              Padding(padding: EdgeInsets.all(32), child: Center(child: Text(tr('Групп пока нет'), style: TextStyle(color: AppTheme.textSecondary)))),
            for (final g in shown)
              InkWell(
                onTap: () => _menu(g),
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                  decoration: const BoxDecoration(border: Border(bottom: BorderSide(color: AppTheme.divider, width: 0.5))),
                  child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  GroupLogo(gid: g.id, name: g.name, hash: g.logoHash, radius: 22),
                  const SizedBox(width: 12),
                  Expanded(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text(g.name, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w700, fontSize: 16)),
                    if (g.topic.isNotEmpty) Text(g.topic, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
                    const SizedBox(height: 4),
                    Wrap(spacing: 12, children: [
                      Row(mainAxisSize: MainAxisSize.min, children: [
                        Icon(g.isOpen ? Icons.lock_open : Icons.lock_outline, size: 14, color: AppTheme.accent),
                        const SizedBox(width: 4),
                        Text(g.isOpen ? tr('Открытая') : tr('По приглашению'), style: const TextStyle(color: AppTheme.accent, fontSize: 12)),
                      ]),
                      Text(trf('{n} уч.', {'n': g.members}), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      if (!g.ownerPresent) Text(tr('владелец отсутствует'), style: TextStyle(color: Colors.orangeAccent, fontSize: 12)),
                      if (g.requested) Text(tr('запрос отправлен'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      if (state.hiddenGroups.contains(g.id)) Text(tr('скрыта'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                    ]),
                  ])),
                  ]),
                ),
              ),
            if (hiddenCount > 0)
              TextButton(
                onPressed: () => setState(() => _showHidden = !_showHidden),
                child: Text(_showHidden ? tr('Не показывать скрытые') : trf('Показать скрытые ({n})', {'n': hiddenCount}), style: const TextStyle(color: AppTheme.textSecondary)),
              ),
          ]);
        },
      ),
    );
  }
}
