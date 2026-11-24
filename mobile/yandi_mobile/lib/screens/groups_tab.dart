import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/group.dart';
import '../models/group_invite.dart';
import '../services/app_state.dart';
import '../services/group_api.dart';
import '../theme.dart';
import '../widgets/secure_text_field.dart';
import '../widgets/group_logo.dart';
import '../widgets/logo_picker.dart';
import 'group_chat_screen.dart';
import '../l10n.dart';

/// Вкладка «Группы»: мои группы, открытые группы узла, вступление по приглашению.
class GroupsTab extends StatefulWidget {
  const GroupsTab({super.key});
  @override
  State<GroupsTab> createState() => _GroupsTabState();
}

class _GroupsTabState extends State<GroupsTab> {
  bool _loaded = false;
  List<DiscoverGroup> _node = const [];   // группы узла, куда можно вступить или попроситься

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _refresh());
  }

  Future<void> _refresh() async {
    final state = context.read<AppState>();
    await state.syncGroups();
    List<DiscoverGroup> node = _node;
    // группы узла, куда вы не вступили, показываются только на «Поиск → Группы»
    node = const [];
    if (mounted) setState(() { _loaded = true; _node = node; });
  }

  Future<void> _joinOrAsk(DiscoverGroup g) async {
    final state = context.read<AppState>();
    final messenger = ScaffoldMessenger.of(context);
    final name = state.myNick;   // имя в группе — ник профиля, другого нет
    if (name.isEmpty) {
      messenger.showSnackBar(SnackBar(content: Text(tr('Сначала задайте ник в профиле'))));
      return;
    }
    try {
      if (g.isOpen) {
        await state.groups!.join(g.id, displayName: name);
        await state.syncGroups();
        messenger.showSnackBar(SnackBar(content: Text(tr('Вы в группе'))));
      } else {
        await state.groups!.requestInvite(g.id, name);
        messenger.showSnackBar(SnackBar(content: Text(tr('Просьба отправлена владельцу группы'))));
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('$e')));
    }
    await _refresh();
  }

  String _roleLabel(String r) => switch (r) { 'owner' => tr('владелец'), 'admin' => tr('админ'), 'moderator' => tr('модератор'), _ => tr('участник') };

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final list = state.groups?.groups ?? const <GroupInfo>[];
    final gone = state.groups?.gone ?? const <(String, String)>[];
    return Scaffold(
      backgroundColor: Colors.transparent,
      floatingActionButton: FloatingActionButton.extended(
        backgroundColor: AppTheme.accent,
        foregroundColor: Colors.black,
        onPressed: () => _menu(context),
        icon: const Icon(Icons.add),
        label: Text(tr('Группа')),
      ),
      body: RefreshIndicator(
        onRefresh: _refresh,
        child: list.isEmpty && gone.isEmpty && _node.isEmpty
            ? ListView(children: [
                SizedBox(
                  height: 420,
                  child: Center(
                    child: Padding(
                      padding: const EdgeInsets.all(32),
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                        const Icon(Icons.groups_outlined, color: AppTheme.textSecondary, size: 64),
                        const SizedBox(height: 16),
                        Text(_loaded ? tr('Групп пока нет') : tr('Загрузка…'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 15)),
                        const SizedBox(height: 8),
                        Text(tr('Создайте свою группу кнопкой «Группа» или вступите по приглашению либо в открытую группу узла.'),
                            textAlign: TextAlign.center, style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                      ]),
                    ),
                  ),
                ),
              ])
            : ListView.separated(
                itemCount: list.length + gone.length + (_node.isEmpty ? 0 : 1 + _node.length),
                separatorBuilder: (_, __) => const Divider(height: 1),
                itemBuilder: (_, i) {
                  if (i >= list.length + gone.length) {
                    final k = i - list.length - gone.length;
                    if (k == 0) {
                      return Padding(
                        padding: EdgeInsets.fromLTRB(16, 16, 16, 6),
                        child: Text(tr('ГРУППЫ УЗЛА'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12, letterSpacing: 1.2)),
                      );
                    }
                    final g = _node[k - 1];
                    return ListTile(
                      leading: GroupLogo(gid: g.id, name: g.name, hash: g.logoHash),
                      title: Text(g.name, style: const TextStyle(color: AppTheme.text)),
                      subtitle: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        if (g.about.isNotEmpty) Text(g.about, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                        if (g.topic.isNotEmpty) Text(trf('Тематика: {t}', {'t': g.topic}), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                        Text(g.isOpen ? tr('открыта') : tr('по приглашению'), style: TextStyle(color: g.isOpen ? AppTheme.accent : AppTheme.incoming, fontSize: 12)),
                      ]),
                      trailing: g.requested
                          ? Text(tr('Запрошено'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12))
                          : TextButton(onPressed: () => _joinOrAsk(g), child: Text(g.isOpen ? tr('Вступить') : tr('Запросить'))),
                    );
                  }
                  if (i >= list.length) {
                    final (gid, name) = gone[i - list.length];
                    return ListTile(
                      leading: const CircleAvatar(backgroundColor: AppTheme.surface, child: Icon(Icons.link_off, color: AppTheme.incoming)),
                      title: Text(name.isEmpty ? tr('Группа') : name, style: const TextStyle(color: AppTheme.textSecondary)),
                      subtitle: Text(tr('Пропала на узле (удалена, вас выгнали или узел восстанавливается). Данные хранятся на телефоне до 30 дней · нажмите, чтобы убрать сейчас'), style: TextStyle(color: AppTheme.incoming, fontSize: 12)),
                      onTap: () async {
                        final ok = await showDialog<bool>(
                          context: context,
                          builder: (ctx) => AlertDialog(
                            backgroundColor: AppTheme.surface,
                            title: Text(tr('Убрать группу с телефона?'), style: TextStyle(color: AppTheme.text)),
                            content: Text(tr('На узле её больше нет. Будут стёрты история и ключи группы на этом телефоне.'), style: TextStyle(color: AppTheme.textSecondary)),
                            actions: [
                              TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'))),
                              TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Убрать'), style: TextStyle(color: AppTheme.incoming))),
                            ],
                          ),
                        );
                        if (ok == true) {
                          await state.groups!.forgetLocal(gid);
                          state.notifyListenersPublic();
                        }
                      },
                    );
                  }
                  final g = list[i];
                  return ListTile(
                    leading: GroupLogo(gid: g.id, name: g.name, hash: g.logoHash),
                    title: Text(g.name, style: const TextStyle(color: AppTheme.text)),
                    subtitle: Text('${g.topic.isEmpty ? '' : trf('Тематика: {t} · ', {'t': g.topic})}${trf('{n} уч. · вы: {role}', {'n': g.members, 'role': _roleLabel(g.role)})}${g.muted ? tr(' · заглушены') : ''}${g.about.isEmpty ? '' : '\n${g.about}'}',
                        style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                    trailing: state.groups!.hasProblem(g.id)
                        ? const Icon(Icons.warning_amber, color: AppTheme.incoming)
                        : g.requests > 0
                            ? Container(
                                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
                                decoration: BoxDecoration(color: AppTheme.incoming, borderRadius: BorderRadius.circular(12)),
                                child: Text('${g.requests}', style: const TextStyle(color: Colors.white, fontSize: 12, fontWeight: FontWeight.bold)),
                              )
                            : null,
                    onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupChatScreen(gid: g.id))),
                  );
                },
              ),
      ),
    );
  }

  void _menu(BuildContext context) {
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(
            leading: const Icon(Icons.group_add_outlined, color: AppTheme.accent),
            title: Text(tr('Создать группу'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Вы станете владельцем: ключ, приглашения и модерация у вас'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); _createSheet(context); },
          ),
          ListTile(
            leading: const Icon(Icons.login, color: AppTheme.accent),
            title: Text(tr('Вступить'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('По приглашению или в открытую группу узла'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); _joinSheet(context); },
          ),
        ]),
      ),
    );
  }

  void _createSheet(BuildContext context) {
    final state = context.read<AppState>();
    final nameCtrl = TextEditingController();
    final topicCtrl = TextEditingController();
    final aboutCtrl = TextEditingController();
    String kind = 'invite';
    String? error;
    bool busy = false;
    String logoB64 = '';
    Uint8List? logoPreview;
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) => Padding(
        padding: EdgeInsets.fromLTRB(16, 16, 16, 16 + MediaQuery.of(ctx).viewInsets.bottom),
        child: SingleChildScrollView(
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(tr('Новая группа'), style: TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold)),
            SizedBox(height: 12),
            LogoPicker(
              current: logoPreview,
              groupName: nameCtrl.text,
              onPicked: (b64, preview) async => setS(() { logoB64 = b64; logoPreview = preview; }),
            ),
            SizedBox(height: 8),
            SecureTextField(controller: nameCtrl, maxLength: 64, style: TextStyle(color: AppTheme.text), decoration: InputDecoration(labelText: tr('Название'))),
            SecureTextField(controller: topicCtrl, maxLength: 40, style: TextStyle(color: AppTheme.text), decoration: InputDecoration(labelText: tr('Тематика (обязательно)'), hintText: tr('например: Садоводство'))),
            Wrap(spacing: 6, runSpacing: 0, children: [
              for (final t in [tr('Семья'), tr('Друзья'), tr('Работа'), tr('Новости'), tr('Политика'), tr('Юмор'), tr('Садоводство'), tr('Технологии'), tr('Музыка'), tr('Игры'), tr('Авто'), tr('Спорт')])
                ActionChip(label: Text(t, style: const TextStyle(fontSize: 12)), visualDensity: VisualDensity.compact, onPressed: () => setS(() => topicCtrl.text = t)),
            ]),
            SecureTextField(controller: aboutCtrl, maxLength: 300, maxLines: 2, style: const TextStyle(color: AppTheme.text), decoration: InputDecoration(labelText: tr('Описание (необязательно)'))),
            const SizedBox(height: 8),
            RadioListTile<String>(
              value: 'invite', groupValue: kind, activeColor: AppTheme.accent, dense: true,
              title: Text(tr('По приглашению'), style: TextStyle(color: AppTheme.text)),
              subtitle: Text(tr('Войти можно только по вашему приглашению'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onChanged: (v) => setS(() => kind = v!),
            ),
            RadioListTile<String>(
              value: 'open', groupValue: kind, activeColor: AppTheme.accent, dense: true,
              title: Text(tr('Открытая для телефонов узла'), style: TextStyle(color: AppTheme.text)),
              subtitle: Text(tr('Любой телефон этого узла видит её и вступает сам'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onChanged: (v) => setS(() => kind = v!),
            ),
            if (error != null) Padding(padding: const EdgeInsets.only(bottom: 8), child: Text(error!, style: const TextStyle(color: AppTheme.incoming))),
            FilledButton(
              style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
              onPressed: busy ? null : () async {
                if (nameCtrl.text.trim().isEmpty) { setS(() => error = tr('Введите название')); return; }
                if (topicCtrl.text.trim().isEmpty) { setS(() => error = tr('Укажите тематику группы')); return; }
                setS(() { busy = true; error = null; });
                try {
                  final gid = await state.groups!.createGroup(nameCtrl.text, topicCtrl.text, aboutCtrl.text, kind, logoB64: logoB64);
                  await state.syncGroups();
                  if (!ctx.mounted) return;
                  Navigator.pop(ctx);
                  if (context.mounted) Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupChatScreen(gid: gid)));
                } catch (e) {
                  setS(() { busy = false; error = '$e'; });
                }
              },
              child: Text(tr('Создать')),
            ),
          ]),
        ),
      )),
    );
  }

  void _joinSheet(BuildContext context) {
    final state = context.read<AppState>();
    final codeCtrl = TextEditingController();
    String? error;
    bool busy = false;
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => StatefulBuilder(builder: (ctx, setS) {
        Future<void> doJoin(String gid, String? code) async {
          final name = state.myNick;   // имя в группе — ник профиля
          if (name.isEmpty) { setS(() => error = tr('Сначала задайте ник в профиле')); return; }
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
              Text(tr('Вступить в группу'), style: TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold)),
              const SizedBox(height: 12),
              SecureTextField(controller: codeCtrl, style: const TextStyle(color: AppTheme.text),
                  decoration: InputDecoration(labelText: tr('Приглашение'), hintText: 'YANDI-GROUP-1:…')),
              const SizedBox(height: 8),
              if (error != null) Padding(padding: const EdgeInsets.only(bottom: 8), child: Text(error!, style: const TextStyle(color: AppTheme.incoming))),
              FilledButton(
                style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                onPressed: busy ? null : () {
                  final inv = GroupInvite.tryParse(codeCtrl.text);
                  if (inv == null) { setS(() => error = tr('Неверное приглашение')); return; }
                  doJoin(inv.gid, inv.code);
                },
                child: Text(tr('Вступить по приглашению')),
              ),
              const SizedBox(height: 12),
              Text(tr('Открытые группы узла: «Поиск» → «Группы».'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ]),
          ),
        );
      }),
    );
  }
}

/// Вступление по приглашению, пришедшему сообщением: спрашивает имя в группе и вступает.
Future<void> joinInvited(BuildContext context, GroupInviteMsg m) async {
  final state = context.read<AppState>();
  final name = state.myNick;   // имя в группе — ник профиля
  if (name.isEmpty) {
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Сначала задайте ник в профиле'))));
    return;
  }
  final messenger = ScaffoldMessenger.of(context);
  try {
    await state.groups!.join(m.invite.gid, inviteCode: m.invite.code.isEmpty ? null : m.invite.code, displayName: name);
    await state.groups!.markInviteSpent(m.invite.gid, m.invite.code);
    await state.syncGroups();
    messenger.showSnackBar(SnackBar(content: Text(tr('Вы в группе — она во вкладке «Группы»'))));
  } on GroupApiError catch (e) {
    // код использован, просрочен или группа удалена: карточка больше не срабатывает
    if (e.status == 403 || e.status == 404) {
      await state.groups!.markInviteSpent(m.invite.gid, m.invite.code, gone: true);
      messenger.showSnackBar(SnackBar(content: Text(tr('Приглашение уже недействительно: оно использовано, просрочено или группы больше нет'))));
    } else if (e.status == 409) {
      await state.groups!.markInviteSpent(m.invite.gid, m.invite.code);
      await state.syncGroups();
      messenger.showSnackBar(SnackBar(content: Text(tr('Вы уже в этой группе'))));
    } else {
      messenger.showSnackBar(SnackBar(content: Text(trf('Не удалось вступить: {e}', {'e': e.message}))));
    }
  } catch (e) {
    messenger.showSnackBar(SnackBar(content: Text(trf('Не удалось вступить: {e}', {'e': e}))));
  }
  state.notifyListenersPublic();
}
