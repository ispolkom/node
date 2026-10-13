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
  bool _cardShared = false;
  List<JoinRequest> _requests = const [];
  List<String> _banned = const [];
  final _nameCtrl = TextEditingController();
  final _topicCtrl = TextEditingController();
  final _aboutCtrl = TextEditingController();
  bool _fieldsLoaded = false;
  String? _error;

  GroupService get _svc => context.read<AppState>().groups!;

  @override
  void dispose() {
    _nameCtrl.dispose();
    _topicCtrl.dispose();
    _aboutCtrl.dispose();
    super.dispose();
  }

  void _fillFields(GroupInfo g) {
    if (_fieldsLoaded) return;
    _nameCtrl.text = g.name;
    _topicCtrl.text = g.topic;
    _aboutCtrl.text = g.about;
    _fieldsLoaded = true;
  }

  Future<void> _saveSettings(GroupInfo g) async {
    final messenger = ScaffoldMessenger.of(context);
    if (_nameCtrl.text.trim().isEmpty) { messenger.showSnackBar(const SnackBar(content: Text('Название не может быть пустым'))); return; }
    if (_topicCtrl.text.trim().isEmpty) { messenger.showSnackBar(const SnackBar(content: Text('Укажите тематику группы'))); return; }
    try {
      await _svc.updateSettings(g.id, name: _nameCtrl.text.trim(), topic: _topicCtrl.text.trim(), about: _aboutCtrl.text.trim());
      messenger.showSnackBar(const SnackBar(content: Text('Сохранено')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('$e')));
    }
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  Future<void> _load() async {
    try {
      final m = await _svc.api.members(widget.gid);
      List<JoinRequest> reqs = const [];
      final gi = _svc.byId(widget.gid);
      if (gi != null && gi.canModerate && gi.kind == 'invite') {
        try { reqs = await _svc.joinRequests(widget.gid); } catch (_) {}
      }
      final shared = await _svc.cardShared(widget.gid);
      if (!mounted) return;
      setState(() {
        _cardShared = shared;
        _members = [for (final x in (m['members'] as List? ?? const [])) GroupMember.fromJson((x as Map).cast<String, dynamic>())];
        _requests = reqs;
        _banned = [for (final b in (m['banned'] as List? ?? const [])) '$b'];
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

  /// Окно действий над участником: заглушить, выгнать, запретить вход; ниже «Администрирование» (только владелец): назначить роль.
  void _memberMenu(GroupInfo g, GroupMember m) {
    if (!_canActOn(g, m)) return;
    final name = _svc.displayName(g.id, m.alias);
    String role = m.role == 'admin' ? 'admin' : (m.role == 'moderator' ? 'moderator' : 'member');
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) => SafeArea(
        child: Padding(
          padding: EdgeInsets.only(bottom: MediaQuery.of(ctx).viewInsets.bottom),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Padding(padding: const EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text(name, style: const TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold))),
            Padding(padding: const EdgeInsets.fromLTRB(16, 0, 16, 8), child: Text('${m.alias} · ${_roleName(m.role)}${m.muted ? ' · заглушён' : ''}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
            ListTile(
              leading: Icon(m.muted ? Icons.volume_up_outlined : Icons.volume_off_outlined, color: AppTheme.textSecondary),
              title: Text(m.muted ? 'Разрешить писать' : 'Заглушить', style: const TextStyle(color: AppTheme.text)),
              onTap: () { Navigator.pop(ctx); _act(m.muted ? 'unmute' : 'mute', m.alias); },
            ),
            ListTile(
              leading: const Icon(Icons.person_remove_outlined, color: AppTheme.incoming),
              title: const Text('Выгнать', style: TextStyle(color: AppTheme.incoming)),
              onTap: () async { Navigator.pop(ctx); if (await _confirm('Выгнать «$name»?')) _act('kick', m.alias); },
            ),
            ListTile(
              leading: const Icon(Icons.block, color: AppTheme.incoming),
              title: const Text('Запретить вход', style: TextStyle(color: AppTheme.incoming)),
              onTap: () async { Navigator.pop(ctx); if (await _confirm('Запретить «$name» вход в группу?')) _act('ban', m.alias); },
            ),
            if (g.canManage) ...[
              const Divider(),
              const Padding(padding: EdgeInsets.fromLTRB(16, 4, 16, 4), child: Text('Администрирование', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
                child: Row(children: [
                  Expanded(
                    child: DropdownButtonFormField<String>(
                      value: role,
                      dropdownColor: AppTheme.surface,
                      decoration: const InputDecoration(labelText: 'Назначить'),
                      items: [
                        const DropdownMenuItem(value: 'member', child: Text('Участник')),
                        const DropdownMenuItem(value: 'moderator', child: Text('Модератор')),
                        if (g.isOwner) const DropdownMenuItem(value: 'admin', child: Text('Админ')),
                      ],
                      onChanged: (v) => setS(() => role = v ?? role),
                    ),
                  ),
                  const SizedBox(width: 12),
                  FilledButton(
                    style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                    onPressed: () {
                      Navigator.pop(ctx);
                      if (role != m.role) _act(switch (role) { 'admin' => 'make_admin', 'moderator' => 'promote', _ => 'demote' }, m.alias);
                    },
                    child: const Text('ОК'),
                  ),
                ]),
              ),
            ],
          ]),
        ),
      )),
    );
  }

  /// Кого я могу трогать: владелец — всех кроме владельца; админ — рядовых и модераторов; модератор — рядовых; себя — никого.
  bool _canActOn(GroupInfo g, GroupMember m) {
    if (m.alias == g.myAlias || m.role == 'owner') return false;
    if (g.isOwner) return true;
    if (g.isAdmin) return m.role == 'member' || m.role == 'moderator';
    if (g.role == 'moderator') return m.role == 'member';
    return false;
  }

  String _roleName(String r) => switch (r) { 'owner' => 'владелец', 'admin' => 'админ', 'moderator' => 'модератор', _ => 'участник' };

  /// Передача владения: выбор участника из списка с прокруткой → «Вы уверены?» (кнопка «Да» оживает через 3 секунды).
  Future<void> _transferOwnership(GroupInfo g) async {
    final messenger = ScaffoldMessenger.of(context);
    final candidates = _members.where((m) => m.alias != g.myAlias).toList();
    String? pick;
    final chosen = await showModalBottomSheet<String>(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          const Padding(padding: EdgeInsets.all(16), child: Text('Кому передать группу', style: TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold))),
          ConstrainedBox(
            constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * 0.5),
            child: ListView(shrinkWrap: true, children: [
              for (final m in candidates)
                RadioListTile<String>(
                  value: m.alias,
                  groupValue: pick,
                  activeColor: AppTheme.accent,
                  title: Text(_svc.displayName(g.id, m.alias), style: const TextStyle(color: AppTheme.text)),
                  subtitle: Text('${m.alias} · ${_roleName(m.role)}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                  onChanged: (v) => setS(() => pick = v),
                ),
            ]),
          ),
          Padding(
            padding: const EdgeInsets.all(16),
            child: Row(children: [
              Expanded(child: TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Отмена'))),
              const SizedBox(width: 12),
              Expanded(child: FilledButton(style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black), onPressed: pick == null ? null : () => Navigator.pop(ctx, pick), child: const Text('ОК'))),
            ]),
          ),
        ]),
      )),
    );
    if (chosen == null || !mounted) return;
    final sure = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => _SureDialog(),
    );
    if (sure != true) return;
    try {
      await _svc.transferOwner(g.id, chosen);
      messenger.showSnackBar(const SnackBar(content: Text('Группа передана. Вы теперь админ.')));
      await _load();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('$e')));
    }
  }

  Future<bool> _confirm(String text) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        content: Text(text, style: const TextStyle(color: AppTheme.text)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Нет')),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Да', style: TextStyle(color: AppTheme.incoming))),
        ],
      ),
    );
    return ok == true;
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
      appBar: AppBar(title: const Text('Настройки группы')),
      body: ListView(children: [
        if (g.canManage) ...[
          Builder(builder: (_) { _fillFields(g); return const SizedBox.shrink(); }),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 0),
            child: Column(children: [
              TextField(controller: _nameCtrl, maxLength: 64, style: const TextStyle(color: AppTheme.text), decoration: const InputDecoration(labelText: 'Название')),
              TextField(controller: _topicCtrl, maxLength: 40, style: const TextStyle(color: AppTheme.text), decoration: const InputDecoration(labelText: 'Тематика')),
              TextField(controller: _aboutCtrl, maxLength: 300, maxLines: 2, style: const TextStyle(color: AppTheme.text), decoration: const InputDecoration(labelText: 'Описание (необязательно)')),
              Align(alignment: Alignment.centerRight, child: FilledButton(style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black), onPressed: () => _saveSettings(g), child: const Text('Сохранить'))),
            ]),
          ),
        ] else ...[
          Padding(padding: const EdgeInsets.fromLTRB(16, 16, 16, 0), child: Text(g.name, style: const TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold))),
          if (g.topic.isNotEmpty) Padding(padding: const EdgeInsets.fromLTRB(16, 4, 16, 0), child: Text('Тематика: ${g.topic}', style: const TextStyle(color: AppTheme.textSecondary))),
          if (g.about.isNotEmpty) Padding(padding: const EdgeInsets.fromLTRB(16, 4, 16, 0), child: Text(g.about, style: const TextStyle(color: AppTheme.text))),
        ],
        ListTile(
          leading: const Icon(Icons.badge_outlined, color: AppTheme.textSecondary),
          title: Text('Ваш псевдоним: ${g.myAlias}', style: const TextStyle(color: AppTheme.text)),
          subtitle: const Text('Свой в каждой группе; узел не знает, кто вы', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
        ),
        if (g.canModerate)
          ListTile(leading: const Icon(Icons.person_add_alt, color: AppTheme.accent), title: const Text('Пригласить из контактов', style: TextStyle(color: AppTheme.text)), onTap: () => _inviteContacts(g)),
        if (g.canModerate && g.kind != 'open')
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
        if (g.canManage) ...[
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
        // автоочистка: владелец выбирает срок; остальные видят действующее правило (чтобы не было сюрпризов)
        if (g.isOwner) ...[
          const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Автоочистка неактивных', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 16),
            child: Text('Рядовые участники, которые ни разу не написали за этот срок, исключаются. Чтение без записей не считается. Владельца, админов, модераторов и заглушённых это не касается.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
          for (final d in const [0, 30, 60, 90])
            RadioListTile<int>(
              key: ValueKey('autoclean-$d'),
              dense: true,
              title: Text(d == 0 ? 'Выключена' : '$d дней', style: const TextStyle(color: AppTheme.text)),
              value: d,
              groupValue: g.autoCleanDays,
              activeColor: AppTheme.accent,
              onChanged: (v) async {
                if (v == null) return;
                try { await _svc.setAutoClean(g.id, v); } catch (err) { if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$err'))); }
                if (mounted) setState(() {});
              },
            ),
        ] else if (g.autoCleanDays > 0)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 0),
            child: Text('В группе включена автоочистка: кто не пишет ${g.autoCleanDays} дней, исключается.', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
        const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Участники', style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold))),
        if (g.canModerate)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 4),
            child: Text(
              g.isOwner
                  ? 'Нажмите на участника: назначить админа или модератора, разжаловать, заглушить, выгнать. Роли — только участникам этой группы.'
                  : g.isAdmin
                      ? 'Нажмите на участника: назначить или снять модератора, заглушить, выгнать. Админов назначает только владелец.'
                      : 'Нажмите на участника: заглушить, выгнать, запретить вход. Роли назначают владелец и админы.',
              style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12),
            ),
          ),
        if (_error != null) Padding(padding: const EdgeInsets.all(16), child: Text(_error!, style: const TextStyle(color: AppTheme.incoming))),
        SwitchListTile(
          key: const Key('card-share'),
          value: _cardShared,
          title: const Text('Показывать мой ник и аватар в этой группе', style: TextStyle(color: AppTheme.text)),
          subtitle: Text(
            _cardShared
                ? 'Участники видят вас как «${state.myNick.isEmpty ? 'без ника' : state.myNick}». Выключите — снова будет только псевдоним ${g.myAlias}.'
                : 'Сейчас вас видят только под псевдонимом ${g.myAlias}. По вашему выбору — под ником и аватаркой из профиля.',
            style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12),
          ),
          onChanged: (on) async {
            try {
              await _svc.setCardShared(widget.gid, on, nick: state.myNick, avatar: state.myAvatarBytes);
              await _load();
            } catch (e) {
              if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
            }
          },
        ),
        // участники: список с прокруткой; нажатие на участника открывает окно действий
        ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.of(context).size.height * 0.42),
          child: ListView(shrinkWrap: true, children: [
            for (final m in _members)
              ListTile(
                dense: true,
                leading: Icon(m.role == 'owner' ? Icons.star : m.role == 'admin' ? Icons.admin_panel_settings_outlined : m.role == 'moderator' ? Icons.shield_outlined : Icons.person_outline, color: m.role == 'member' ? AppTheme.textSecondary : AppTheme.accent),
                title: Text(state.groups!.displayName(g.id, m.alias) + (m.alias == g.myAlias ? ' (вы)' : ''), style: const TextStyle(color: AppTheme.text)),
                subtitle: Text('${_roleName(m.role)} · ${m.alias}${m.muted ? ' · заглушён' : ''}${m.hasKey || m.role == 'owner' ? '' : ' · ждёт ключ'}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                trailing: g.canModerate && _canActOn(g, m) ? const Icon(Icons.chevron_right, color: AppTheme.textSecondary, size: 18) : null,
                onTap: g.canModerate && _canActOn(g, m) ? () => _memberMenu(g, m) : null,
              ),
          ]),
        ),
        if (g.canModerate && _banned.isNotEmpty) ...[
          Padding(padding: const EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Запрещён вход (${_banned.length})', style: const TextStyle(color: AppTheme.incoming, fontWeight: FontWeight.bold))),
          for (final al in _banned)
            ListTile(dense: true, title: Text(al, style: const TextStyle(color: AppTheme.text)), trailing: TextButton(onPressed: () => _act('unban', al), child: const Text('Разрешить вход'))),
        ],
        if (g.isOwner && _members.length > 1) ...[
          const Padding(padding: EdgeInsets.fromLTRB(16, 16, 16, 4), child: Text('Передача владения', style: TextStyle(color: AppTheme.incoming, fontWeight: FontWeight.bold))),
          const Padding(padding: EdgeInsets.symmetric(horizontal: 16), child: Text('Передайте группу другому участнику. Вы станете админом.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
          ListTile(leading: const Icon(Icons.swap_horiz, color: AppTheme.incoming), title: const Text('Передать группу…', style: TextStyle(color: AppTheme.incoming)), onTap: () => _transferOwnership(g)),
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

/// «Вы уверены?» с кнопкой «Да», которая оживает через 3 секунды (чтобы не нажать по ошибке).
class _SureDialog extends StatefulWidget {
  @override
  State<_SureDialog> createState() => _SureDialogState();
}

class _SureDialogState extends State<_SureDialog> {
  bool _ready = false;

  @override
  void initState() {
    super.initState();
    Future.delayed(const Duration(seconds: 3), () { if (mounted) setState(() => _ready = true); });
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: const Text('Вы уверены?', style: TextStyle(color: AppTheme.text)),
        content: const Text('Вы уверены, что хотите передать права на группу? Это действие необратимо без согласия нового владельца.', style: TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(context, false), child: const Text('Нет')),
          TextButton(onPressed: _ready ? () => Navigator.pop(context, true) : null, child: Text(_ready ? 'Да' : 'Да (3 с)', style: TextStyle(color: _ready ? AppTheme.incoming : AppTheme.textSecondary))),
        ],
      );
}
