import 'dart:async';
import 'package:flutter/material.dart';
import 'contact_avatar.dart';
import 'preferences.dart';
import 'nick.dart';
import 'transport/api.dart';

/// Страница «Поиск»: три карточки — «Гости», «Пользователи» (авторизованные) и «Группы», в каждой количество.
/// Пользователи: сопряжённые с узлом устройства (не удалённые узлом за бездействие) со статусом «Онлайн» / «Не в сети».
/// Группы: все группы узла — название, тематика, открытая или по приглашению, число участников.
class SearchFeature extends StatefulWidget {
  final Api api;
  final bool Function() isActive;
  final void Function(ApiFailure failure) onFailure;
  const SearchFeature({super.key, required this.api, required this.isActive, required this.onFailure});
  @override
  State<SearchFeature> createState() => _SearchFeatureState();
}

class _SearchFeatureState extends State<SearchFeature> {
  List<Map<String, dynamic>> users = [], groups = [];
  final Set<String> hiddenUsers = {}, hiddenGroups = {};
  String? view; // null — карточки, 'users', 'groups'
  bool showHidden = false, loading = true;
  String? error, notice;
  Timer? timer;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    refresh();
    timer = Timer.periodic(const Duration(seconds: 10), (_) => refresh());
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  static List<Map<String, dynamic>> listOf(Object? v) =>
      v is List ? [for (final x in v) if (x is Map) x.cast<String, dynamic>()] : const [];

  Future<void> refresh() async {
    if (!active) return;
    // люди и группы грузятся независимо: сбой каталога групп не прячет людей, и наоборот
    String? err;
    List<Map<String, dynamic>>? people;
    List<Map<String, dynamic>>? catalog;
    await Future.wait([
      () async {
        try {
          people = listOf((await widget.api.request('/api/mobile/directory'))['devices']);
        } catch (e) {
          err = e is ApiFailure ? e.message : 'Не удалось получить данные узла.';
        }
      }(),
      () async {
        try {
          catalog = listOf((await widget.api.request('/api/groups'))['catalog']);
        } catch (e) {
          err ??= e is ApiFailure ? e.message : 'Не удалось получить данные узла.';
        }
      }(),
    ]);
    if (!active) return;
    setState(() {
      final p = people;
      if (p != null) {
        users = p;
        hiddenUsers
          ..clear()
          ..addAll([for (final u in p) if (u['status'] == 'hidden') '${u['peer_id']}']);
        showHidden = true;
      }
      final cg = catalog;
      if (cg != null) groups = cg;
      loading = false;
      error = err;
    });
  }

  Future<String?> askName(String title) async {
    final ctrl = TextEditingController();
    final r = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: UiText(title),
        content: TextField(controller: ctrl, autofocus: true, decoration: InputDecoration(labelText: tr(context, 'Имя'))),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const UiText('Отмена')),
          TextButton(onPressed: () => Navigator.pop(ctx, ctrl.text.trim()), child: const UiText('Продолжить')),
        ],
      ),
    );
    ctrl.dispose();
    return r;
  }

  Future<void> userMenu(Map<String, dynamic> u) async {
    final id = '${u['peer_id']}';
    final name = '${u['name'] ?? ''}';
    final hidden = hiddenUsers.contains(id);
    final pick = await showDialog<String>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: Text(name.isEmpty ? id.substring(0, 12) : name),
        children: [
          if (u['saved'] == true)
            SimpleDialogOption(onPressed: () => Navigator.pop(ctx, 'remove'), child: const UiText('Удалить из контактов'))
          else
            SimpleDialogOption(onPressed: () => Navigator.pop(ctx, 'add'), child: const UiText('Добавить в контакты')),
          SimpleDialogOption(onPressed: () => Navigator.pop(ctx, 'hide'), child: UiText(hidden ? 'Показать в чате' : 'Скрыть в чате')),
        ],
      ),
    );
    if (!active || pick == null) return;
    if (pick == 'remove') {
      // «Удалить контакт»: человек выходит из контактов и остаётся в общем списке людей узла (не скрывается)
      try {
        await widget.api.request('/api/mobile/phone-action', body: {'peer': id, 'action': 'remove'});
        await widget.api.request('/api/mobile/phone-action', body: {'peer': id, 'action': 'unhide'});
        if (active) setState(() => notice = 'Удалён из контактов: остаётся в списке пользователей.');
        await refresh();
      } catch (e) {
        if (active) setState(() => notice = 'Не получилось: ${e is ApiFailure ? e.message : e}');
      }
      return;
    }
    if (pick == 'hide') {
      try {
        await widget.api.request('/api/mobile/phone-action', body: {'peer': id, 'action': hidden ? 'unhide' : 'hide'});
        await refresh();
      } catch (e) {
        if (active) setState(() => notice = 'Не получилось: ${e is ApiFailure ? e.message : e}');
      }
      return;
    }
    // добавление: сразу предлагаем дать имя (аватар не меняется — он принадлежит владельцу аккаунта)
    final given = await askName('Как назвать контакт?');
    if (!active || given == null) return;
    try {
      await widget.api.request('/api/mobile/phone-action', body: {'peer': id, 'action': 'add'});
      if (given.isNotEmpty) {
        await widget.api.request('/api/contacts', body: {'peer_id': id, 'name': given});
      }
      if (active) setState(() => notice = 'Добавлено в контакты.');
      await refresh();
    } catch (e) {
      if (active) setState(() => notice = 'Не получилось добавить: ${e is ApiFailure ? e.message : e}');
    }
  }

  Future<void> groupMenu(Map<String, dynamic> g) async {
    final gid = '${g['id']}';
    final open = g['kind'] == 'open';
    final hidden = hiddenGroups.contains(gid);
    final pick = await showDialog<String>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: Text('${g['name']}'),
        children: [
          SimpleDialogOption(
            onPressed: g['requested'] == true ? null : () => Navigator.pop(ctx, 'join'),
            child: UiText(g['requested'] == true ? 'Запрос уже отправлен' : (open ? 'Вступить' : 'Попросить вход')),
          ),
          SimpleDialogOption(onPressed: () => Navigator.pop(ctx, 'hide'), child: UiText(hidden ? 'Показать' : 'Скрыть')),
        ],
      ),
    );
    if (!active || pick == null) return;
    if (pick == 'hide') {
      setState(() => hidden ? hiddenGroups.remove(gid) : hiddenGroups.add(gid));
      return;
    }
    if (g['owner_present'] == false) {
      setState(() => notice = 'Владелец группы отсутствует: ключ выдать некому, вступить нельзя.');
      return;
    }
    // имя в группе — ник профиля (одно имя на человека); отдельное имя не спрашивается
    final name = await loadNick(widget.api);
    if (!active) return;
    if (name == null) {
      setState(() => notice = 'Сначала задайте ник в профиле.');
      return;
    }
    try {
      await widget.api.request('/api/groups/$gid/${open ? 'join' : 'request'}', body: {'name': name});
      if (active) {
        setState(() => notice = open ? 'Вы в группе.' : 'Запрос отправлен владельцу и модераторам.');
      }
      await refresh();
    } catch (e) {
      if (active) setState(() => notice = 'Не получилось: ${e is ApiFailure ? e.message : e}');
    }
  }

  Widget hubCard(String title, String subtitle, int count, String target) => Card(
    child: InkWell(
      onTap: () => setState(() {
        view = target;
        notice = null;
      }),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  UiText(title, style: const TextStyle(fontSize: 20, fontWeight: FontWeight.bold)),
                  UiText(subtitle),
                ],
              ),
            ),
            Text('$count', style: const TextStyle(fontSize: 30, fontWeight: FontWeight.bold)),
          ],
        ),
      ),
    ),
  );

  Widget statusBadge(bool online) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      Container(
        width: 10,
        height: 10,
        decoration: BoxDecoration(shape: BoxShape.circle, color: online ? Colors.green : Colors.red),
      ),
      const SizedBox(width: 6),
      UiText(online ? 'Онлайн' : 'Не в сети'),
    ],
  );

  bool isGuestLevel(Map<String, dynamic> u) => u['role'] == 'guest'; // приглашённый по цепочке доверия — пользователь

  List<Widget> usersView({required bool guests}) {
    final all = [for (final u in users) if (isGuestLevel(u) == guests) u];
    final shown = [for (final u in all) if (showHidden || !hiddenUsers.contains('${u['peer_id']}')) u];
    return [
      for (final u in shown)
        Card(
          child: ListTile(
            onTap: () => userMenu(u),
            leading: ContactAvatar(api: widget.api, avatarId: '${u['peer_id']}', name: '${u['name'] ?? ''}'),
            title: Text('${u['name'] ?? ''}'),
            subtitle: Wrap(spacing: 12, children: [statusBadge(u['online'] == true), if (u['saved'] == true) const UiText('в контактах')]),
            trailing: hiddenUsers.contains('${u['peer_id']}') ? const UiText('скрыт') : const Icon(Icons.more_vert),
          ),
        ),
      if (shown.isEmpty) UiText(guests ? 'Гостей пока нет.' : 'Пока никого нет.'),
      if (all.any((u) => hiddenUsers.contains('${u['peer_id']}')))
        TextButton(
          onPressed: () => setState(() => showHidden = !showHidden),
          child: UiText(showHidden ? 'Не показывать скрытых' : 'Показать скрытых (${hiddenUsers.length})'),
        ),
    ];
  }

  List<Widget> groupsView() {
    final shown = [for (final g in groups) if (showHidden || !hiddenGroups.contains('${g['id']}')) g];
    Widget tile(Map<String, dynamic> g) => Card(
          child: ListTile(
            onTap: () => groupMenu(g),
            title: Text('${g['name'] ?? ''}'),
            subtitle: Wrap(
              spacing: 12,
              children: [
                if ('${g['topic'] ?? ''}'.isNotEmpty) Text('${g['topic']}'),
                UiText(g['kind'] == 'open' ? 'Открытая' : 'По приглашению'),
                UiText('${g['members'] ?? 0} уч.'),
                if (g['owner_present'] == false) const UiText('владелец отсутствует'),
              ],
            ),
            trailing: const Icon(Icons.more_vert),
          ),
        );
    Iterable<Widget> section(String title, Iterable<Map<String, dynamic>> items) => items.isEmpty
        ? const []
        : [
            Padding(padding: const EdgeInsets.only(top: 12, bottom: 4), child: UiText('$title (${items.length})', style: const TextStyle(fontWeight: FontWeight.bold))),
            for (final g in items) tile(g),
          ];
    return [
      ...section('Открытые группы', shown.where((g) => g['kind'] == 'open')),
      ...section('Группы по приглашению', shown.where((g) => g['kind'] != 'open')),
      if (shown.isEmpty) const UiText('Групп пока нет.'),
      if (hiddenGroups.isNotEmpty)
        TextButton(
          onPressed: () => setState(() => showHidden = !showHidden),
          child: UiText(showHidden ? 'Не показывать скрытые' : 'Показать скрытые (${hiddenGroups.length})'),
        ),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final guestCount = users.where(isGuestLevel).length;
    final userCount = users.where((u) => !isGuestLevel(u)).length;
    final visibleGroups = groups.where((g) => !hiddenGroups.contains('${g['id']}')).length;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (loading) const LinearProgressIndicator(),
        if (error != null) UiText(error!),
        if (notice != null) UiText(notice!),
        if (view == null) ...[
          KeyedSubtree(key: const ValueKey('hub-guests'), child: hubCard('Гости', 'Вошли без кода: ники, статус', guestCount, 'guests')),
          KeyedSubtree(key: const ValueKey('hub-users'), child: hubCard('Пользователи', 'Авторизованные на узле: ники, аватарки, статус', userCount, 'users')),
          KeyedSubtree(key: const ValueKey('hub-groups'), child: hubCard('Группы', 'Все группы узла: вступить или попросить приглашение', visibleGroups, 'groups')),
        ] else ...[
          Row(
            children: [
              TextButton.icon(
                onPressed: () => setState(() {
                  view = null;
                  showHidden = false;
                }),
                icon: const Icon(Icons.arrow_back),
                label: const UiText('Назад'),
              ),
              UiText(
                view == 'guests' ? 'Гости ($guestCount)' : view == 'users' ? 'Пользователи ($userCount)' : 'Группы ($visibleGroups)',
                style: const TextStyle(fontSize: 20),
              ),
            ],
          ),
          ...(view == 'groups' ? groupsView() : usersView(guests: view == 'guests')),
        ],
      ],
    );
  }
}
