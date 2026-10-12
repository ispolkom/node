import 'preferences.dart';
import 'dart:io';
import 'attachments.dart';
import 'platform_files.dart';
import 'domain/file_repository.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'domain/group_repository.dart';
import 'domain/shell_repository.dart';
import 'domain/states.dart';
import 'transport/api.dart';

class GroupsFeature extends StatefulWidget {
  final Api api;
  final bool Function() isActive;
  final void Function(ApiFailure) onFailure;
  final VoidCallback onLocked;
  const GroupsFeature({
    super.key,
    required this.api,
    required this.isActive,
    required this.onFailure,
    required this.onLocked,
  });
  @override
  State<GroupsFeature> createState() => _GroupsFeatureState();
}

class _GroupsFeatureState extends State<GroupsFeature> {
  late final repo = GroupRepository(widget.api);
  final composer = TextEditingController();
  final drafts = <String, String>{};
  List<Map<String, dynamic>> mine = [], catalog = [], gone = [], admin = [];
  Map<String, dynamic>? feed, info;
  String? selected, error, invitation;
  String tab = 'Свои группы';
  bool loading = true, refreshing = false, busy = false;
  int revision = 0;
  Timer? timer;
  File? picked;
  int sentBytes = 0, fileSize = 0;
  bool choosing = false;
  bool get active => mounted && widget.isActive();
  Map<String, dynamic> get group =>
      (info?['group'] ?? feed?['group'] ?? <String, dynamic>{})
          as Map<String, dynamic>;
  String get role => '${group['role']}';
  bool get staff => const ['owner', 'admin', 'moderator'].contains(role);
  bool get manager => role == 'owner' || role == 'admin';
  List<Map<String, dynamic>> rows(Map<String, dynamic>? value, String key) =>
      value?[key] is List ? ShellRepository.list(value!, key) : [];

  @override
  void initState() {
    super.initState();
    refresh();
    timer = Timer.periodic(const Duration(seconds: 4), (_) => refresh());
  }

  @override
  void dispose() {
    revision++;
    picked = null;
    PlatformFiles.cancel().catchError((Object _) {});
    timer?.cancel();
    composer.clear();
    composer.dispose();
    drafts.clear();
    mine = catalog = gone = admin = [];
    feed = info = null;
    invitation = selected = null;
    super.dispose();
  }

  void failure(Object e) {
    final f = e is ApiFailure ? e : const ApiFailure('protocol');
    if (const ['disconnected', 'timeout', 'protocol'].contains(f.kind)) {
      revision++;
      widget.onFailure(f);
    } else if (active) {
      setState(() => error = f.message);
    }
  }

  Future<void> refresh() async {
    if (!active || refreshing || busy) return;
    refreshing = true;
    final rev = revision, id = selected;
    try {
      final list = await repo.groups();
      if (!active || rev != revision) return;
      final own = ShellRepository.list(list, 'groups');
      Map<String, dynamic>? nextFeed, nextInfo;
      final stillMember = id != null && own.any((g) => g['id'] == id);
      if (stillMember) {
        nextFeed = await repo.feed(id);
        if (!active || rev != revision) return;
        nextInfo = await repo.info(id);
        if (!active || rev != revision) return;
      }
      List<Map<String, dynamic>> nodeGroups = admin;
      if (tab == 'Управление узла') {
        nodeGroups = ShellRepository.list(
          await widget.api.request('/api/mobile/groups'),
          'groups',
        );
        if (!active || rev != revision) return;
      }
      setState(() {
        mine = own;
        catalog = ShellRepository.list(list, 'catalog');
        gone = ShellRepository.list(list, 'gone');
        admin = nodeGroups;
        feed = nextFeed;
        info = nextInfo;
        error = null;
        if (id != null && !stillMember) {
          drafts.remove(id);
          selected = null;
          composer.clear();
          invitation = null;
        }
      });
    } catch (e) {
      if (active && rev == revision) {
        setState(() {
          feed = info = null;
        });
        failure(e);
      }
    } finally {
      refreshing = false;
      if (active && rev == revision) setState(() => loading = false);
    }
  }

  Future<void> choose(String? id, {String? nextTab}) async {
    if (!active || busy) return;
    if (selected != null) drafts[selected!] = composer.text;
    revision++;
    setState(() {
      picked = null;
      sentBytes = fileSize = 0;
      selected = id;
      tab = nextTab ?? tab;
      composer.text = drafts[id] ?? '';
      feed = info = null;
      invitation = error = null;
      loading = true;
    });
    while (active && refreshing) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    if (active) await refresh();
  }

  Future<void> mutate(
    Future<void> Function() action, {
    VoidCallback? success,
  }) async {
    if (!active || busy) return;
    final rev = revision;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final vault = VaultState.fromJson(
        await widget.api.request('/api/vault/status'),
      );
      if (!active || rev != revision) return;
      if (vault.hasError || vault.configured && !vault.unlocked) {
        revision++;
        widget.onLocked();
        return;
      }
      await action();
      if (!active || rev != revision) return;
      success?.call();
    } catch (e) {
      if (active && rev == revision) failure(e);
    } finally {
      if (mounted) setState(() => busy = false);
    }
    while (active && refreshing) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    if (active && rev == revision && error == null) await refresh();
  }

  Future<bool> confirm(
    String title, {
    String? detail,
    bool transfer = false,
  }) async {
    final rev = revision;
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => GroupConfirm(
        title: title,
        detail: detail,
        countdown: transfer ? 3 : 0,
      ),
    );
    return active && rev == revision && ok == true;
  }

  Future<List<String>?> form(
    String title,
    List<String> labels, {
    List<String>? initial,
    bool secret = false,
    bool creating = false,
  }) async {
    final rev = revision;
    final value = await showDialog<List<String>>(
      context: context,
      builder: (_) => GroupForm(
        title: title,
        labels: labels,
        initial: initial,
        secret: secret,
        creating: creating,
      ),
    );
    return active && rev == revision ? value : null;
  }

  Future<void> create() async {
    final values = await form('Создать группу', [
      'Название',
      'Тематика',
      'Описание',
    ], creating: true);
    if (values == null) return;
    String? id;
    await mutate(() async {
      id = await repo.create(values[0], values[1], values[2], values[3]);
    });
    if (active && id != null) await choose(id);
  }

  Future<void> join({Map<String, dynamic>? entry}) async {
    final values = await form(
      'Вступить в группу',
      entry == null
          ? ['Приглашение YANDI-GROUP-1 или ID', 'Ваше имя']
          : ['Ваше имя'],
    );
    if (values == null) return;
    var id = '${entry?['id'] ?? values[0].trim()}';
    String? code;
    final name = values.last.trim();
    if (entry == null && id.startsWith('YANDI-GROUP-1:')) {
      final parts = id.split(':');
      if (parts.length != 3) {
        setState(() => error = 'Неверное приглашение.');
        return;
      }
      id = parts[1].toLowerCase();
      code = parts[2];
    }
    final request = entry != null && entry['kind'] == 'invite';
    await mutate(() async {
      await repo.act(id, request ? 'request' : 'join', {
        'name': name,
        if (code != null) 'code': code,
      });
    });
  }

  Future<void> mod(String action, Map<String, dynamic> member) async {
    final id = selected!;
    if (!await confirm(
      'Подтвердить: ${labels[action]}?',
      detail: '${member['name']} · ${member['alias']}',
    ))
      return;
    await mutate(() async {
      await repo.act(id, 'mod', {'action': action, 'alias': member['alias']});
    });
  }

  Future<void> send() async {
    final id = selected;
    if (id == null || busy || !canSend || composer.text.trim().isEmpty) return;
    final text = composer.text;
    await mutate(
      () => repo.send(id, text),
      success: () {
        composer.clear();
        drafts.remove(id);
      },
    );
  }

  bool get canSend =>
      feed != null &&
      feed!['has_key'] == true &&
      feed!['problem'] != true &&
      feed!['damaged'] != true &&
      group['muted'] != true;
  Widget button(String label, VoidCallback callback) => TextButton(
    onPressed: busy || loading ? null : callback,
    child: Text(label),
  );
  static const labels = {
    'mute': 'Запретить писать',
    'unmute': 'Разрешить писать',
    'kick': 'Исключить',
    'ban': 'Заблокировать',
    'unban': 'Снять блокировку',
    'promote': 'Назначить модератором',
    'demote': 'Сделать участником',
    'make_admin': 'Назначить админом',
  };

  Widget participants() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const UiText('Участники', style: TextStyle(fontSize: 22)),
      for (final m in rows(info, 'members'))
        Card(
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${m['name']} · ${m['role']}${m['muted'] == true ? ' · писать запрещено' : ''}',
                ),
                SelectableText('${m['alias']}'),
                Wrap(
                  children: [
                    if (GroupRepository.canTouch(role, '${m['role']}')) ...[
                      for (final action in [
                        m['muted'] == true ? 'unmute' : 'mute',
                        'kick',
                        'ban',
                        if (manager) ...['promote', 'demote'],
                        if (role == 'owner') 'make_admin',
                      ])
                        button(labels[action]!, () => mod(action, m)),
                    ],
                    if (role == 'owner' &&
                        m['role'] != 'owner' &&
                        m['has_key'] == true)
                      button('Передать владение', () async {
                        final id = selected!;
                        if (!await confirm(
                          'Передать владение?',
                          detail:
                              'Вы станете админом. Новый владелец: ${m['name']}',
                          transfer: true,
                        ))
                          return;
                        await mutate(() async {
                          await repo.act(id, 'transfer', {'alias': m['alias']});
                        });
                      }),
                  ],
                ),
              ],
            ),
          ),
        ),
      if (staff) ...[
        button('Создать приглашение', () async {
          final id = selected!;
          await mutate(() async {
            final result = await repo.act(id, 'invite-code');
            if (active) invitation = result['invite'] as String?;
          });
        }),
        if (invitation != null) SelectableText(invitation!),
        button('Пригласить телефон', () async {
          final id = selected!;
          final rev = revision;
          List<Map<String, dynamic>>? devices;
          await mutate(() async {
            devices = ShellRepository.list(
              await widget.api.request('/api/groups/devices'),
              'devices',
            );
          });
          if (!active || revision != rev || devices == null) return;
          final peer = await showDialog<String>(
            context: context,
            builder: (ctx) => SimpleDialog(
              title: const UiText('Пригласить телефон'),
              children: [
                if (devices!.isEmpty) const UiText('Устройств нет.'),
                for (final d in devices!)
                  SimpleDialogOption(
                    onPressed: () => Navigator.pop(ctx, d['id']),
                    child: Text('${d['name']}'),
                  ),
                TextButton(
                  onPressed: () => Navigator.pop(ctx),
                  child: const UiText('Отмена'),
                ),
              ],
            ),
          );
          if (!active || rev != revision || peer == null) return;
          await mutate(() async {
            if (!RegExp(r'^[a-f0-9]{64}$').hasMatch(peer))
              throw const ApiFailure('protocol');
            await repo.act(id, 'invite-device', {'peer': peer});
          });
        }),
        for (final r in rows(info, 'requests'))
          ListTile(
            title: Text('Просьба: ${r['name']}'),
            subtitle: Wrap(
              children: [
                for (final answer in [
                  'Пригласить',
                  'Пропустить',
                  'Больше не напоминать',
                ])
                  button(answer, () async {
                    final id = selected!;
                    await mutate(() async {
                      await repo.act(id, 'answer', {
                        'rid': r['id'],
                        'accept': answer == 'Пригласить',
                        'never': answer == 'Больше не напоминать',
                      });
                    });
                  }),
              ],
            ),
          ),
        for (final b in rows(info, 'banned'))
          button('Снять блокировку ${b['alias']}', () => mod('unban', b)),
        for (final log in rows(info, 'modlog').reversed.take(20))
          Text('${log['ts']} · ${log['action']} · ${log['target'] ?? ''}'),
      ],
    ],
  );
  Future<void> pickFile() async {
    final rev = revision;
    choosing = true;
    try {
      final path = await PlatformFiles.choose();
      if (active && rev == revision && path != null)
        setState(() {
          picked = File(path);
          sentBytes = fileSize = 0;
        });
    } catch (_) {
      if (active)
        setState(() => error = 'Не удалось открыть диалог выбора файла.');
    } finally {
      if (mounted) setState(() => choosing = false);
    }
  }

  Future<void> sendFile() async {
    final file = picked, id = selected, rev = revision;
    if (file == null || id == null || busy || !canSend) return;
    final kind = switch (FileRepository.mime(file.path).split('/').first) {
      'image' => 2,
      'audio' => 4,
      'video' => 5,
      _ => 3,
    };
    if ((feed?['deny'] as List? ?? []).contains(kind)) {
      setState(() => error = 'В этой группе запрещён такой вид файлов.');
      return;
    }
    await mutate(() async {
      if (widget.api is! GroupFileApi) throw const ApiFailure('protocol');
      final result = await (widget.api as GroupFileApi).uploadGroup(
        '${repo.path(id, 'upload')}?name=${Uri.encodeComponent(file.uri.pathSegments.last)}',
        file,
        isActive: () => active && revision == rev,
        progress: (sent, size) {
          if (active && revision == rev)
            setState(() {
              sentBytes = sent;
              fileSize = size;
            });
        },
      );
      if (result['seq'] is! int || result['seq'] <= 0)
        throw const ApiFailure('protocol');
    }, success: () => picked = null);
  }

  bool canDelete(Map<String, dynamic> message) {
    if (!staff) return false;
    if (message['from'] == group['my_alias']) return true;
    final authors = rows(
      info,
      'members',
    ).where((m) => m['alias'] == message['from']);
    return authors.isEmpty ||
        GroupRepository.canTouch(role, '${authors.first['role']}');
  }

  Future<void> deleteMessage(Map<String, dynamic> message) async {
    final id = selected!;
    if (!await confirm(
      'Удалить сообщение группы?',
      detail: 'Запись будет удалена для участников группы.',
    ))
      return;
    await mutate(() async {
      await repo.act(id, 'mod', {
        'action': 'delete_entry',
        'seq': message['seq'],
      });
    });
  }

  Widget conversation() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(
        '${group['name'] ?? 'Загрузка группы…'}',
        style: const TextStyle(fontSize: 22),
      ),
      const UiText('История не старше календарного года.'),
      if (feed?['damaged'] == true) const UiText('Повреждена лента на диске.'),
      if (feed?['problem'] == true)
        const UiText('Ключ владельца изменился: доверие не подтверждено.'),
      if (feed != null && feed!['has_key'] != true)
        const UiText('Ждём ключ группы.'),
      if (group['muted'] == true) const UiText('Вам запрещено писать.'),
      if (feed?['deny'] is List && (feed!['deny'] as List).isNotEmpty)
        Text(
          'Запрещено: ${(feed!['deny'] as List).map((k) => const {2: 'изображения', 3: 'файлы', 4: 'аудио', 5: 'видео', 6: 'ссылки'}[k] ?? '$k').join(', ')}',
        ),
      if (feed != null && rows(feed, 'messages').isEmpty)
        const UiText('История пуста.'),
      for (final m in rows(feed, 'messages'))
        Card(
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(m['mine'] == true ? 'Вы' : '${m['author']}'),
                if (m['attachment'] is Map)
                  AttachmentView(
                    key: ValueKey('$selected-${m['seq']}'),
                    api: widget.api,
                    attachment: {
                      ...Map<String, dynamic>.from(m['attachment']),
                      'mime_type': FileRepository.mime(
                        '${m['attachment']['filename']}',
                      ),
                    },
                    contentPath: repo.path(selected!, 'file/${m['seq']}'),
                    isActive: () => active,
                    mutate: (a) => mutate(a),
                    busy: busy,
                  )
                else
                  SelectableText(
                    m['text'] == null
                        ? tr(context, 'Вложение (недоступно)')
                        : '${m['text']}',
                  ),
                Text(
                  '${DateTime.fromMillisecondsSinceEpoch((m['ts'] as num).toInt()).toLocal()} · №${m['seq']}',
                ),
                if (canDelete(m))
                  button(
                    'Удалить сообщение №${m['seq']}',
                    () => deleteMessage(m),
                  ),
              ],
            ),
          ),
        ),
      Focus(
        onKeyEvent: (_, event) {
          if (event is KeyDownEvent &&
              event.logicalKey == LogicalKeyboardKey.enter &&
              (HardwareKeyboard.instance.isControlPressed ||
                  HardwareKeyboard.instance.isMetaPressed)) {
            send();
            return KeyEventResult.handled;
          }
          return KeyEventResult.ignored;
        },
        child: TextField(
          controller: composer,
          enabled: !busy && canSend,
          minLines: 2,
          maxLines: 6,
          decoration: const InputDecoration(labelText: 'Сообщение группе'),
        ),
      ),
      FilledButton(
        onPressed: !busy && canSend ? send : null,
        child: const UiText('Отправить группе'),
      ),
      Wrap(
        children: [
          TextButton(
            onPressed: busy || choosing || !canSend ? null : pickFile,
            child: const UiText('Выбрать файл'),
          ),
          if (picked != null) ...[
            Text(picked!.uri.pathSegments.last),
            TextButton(
              onPressed: busy ? null : () => setState(() => picked = null),
              child: const UiText('Убрать файл'),
            ),
            FilledButton(
              onPressed: busy || !canSend ? null : sendFile,
              child: const UiText('Отправить файл'),
            ),
          ],
        ],
      ),
      if (fileSize > 0) ...[
        LinearProgressIndicator(value: sentBytes / fileSize),
        UiText(
          'Передано демону: $sentBytes / $fileSize байт. Доставка подтверждается отдельно.',
        ),
      ],
      if (info != null) ...[
        participants(),
        if (manager) ...[
          button('Изменить настройки группы', () async {
            final id = selected!;
            final value = await form(
              'Настройки группы',
              ['Название', 'Тематика', 'Описание'],
              initial: [
                '${group['name']}',
                '${group['topic']}',
                '${group['about'] ?? ''}',
              ],
            );
            if (value == null) return;
            await mutate(() async {
              await repo.act(id, 'settings', {
                'name': value[0],
                'topic': value[1],
                'about': value[2],
              });
            });
          }),
          button('Правила группы', () async {
            final id = selected!, rev = revision;
            final deny = await showDialog<List<int>>(
              context: context,
              builder: (_) => GroupRules(
                initial: (info?['deny'] as List? ?? []).cast<int>(),
              ),
            );
            if (!active || rev != revision || deny == null) return;
            await mutate(() async {
              await repo.act(id, 'rules', {'deny': deny});
            });
          }),
        ],
        if (role != 'owner')
          button('Покинуть группу', () async {
            final id = selected!;
            if (!await confirm('Покинуть группу?')) return;
            await mutate(
              () async {
                await repo.act(id, 'leave');
              },
              success: () {
                drafts.remove(id);
                composer.clear();
              },
            );
          }),
      ],
    ],
  );
  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (loading || busy) const LinearProgressIndicator(),
      if (error != null)
        Text(error!, style: const TextStyle(color: Colors.red)),
      Wrap(
        children: [
          for (final label in ['Свои группы', 'Каталог', 'Управление узла'])
            button(label, () => choose(null, nextTab: label)),
          button('Обновить группы', refresh),
        ],
      ),
      Wrap(
        children: [
          button('Создать группу', create),
          button('Вступить по приглашению', () => join()),
        ],
      ),
      if (tab == 'Свои группы') ...[
        if (!loading && mine.isEmpty)
          const UiText('Вы ещё не состоите в группах.'),
        for (final g in mine)
          button('${g['name']} · ${g['role']}', () => choose('${g['id']}')),
        for (final g in gone)
          ListTile(
            title: Text('${g['name']}: удалена или вы исключены'),
            subtitle: button('Забыть ${g['name']}', () async {
              final id = '${g['id']}';
              if (!await confirm('Забыть группу?')) return;
              await mutate(() async {
                await repo.act(id, 'forget');
              });
            }),
          ),
      ],
      if (tab == 'Каталог') ...[
        if (!loading && catalog.isEmpty) const UiText('Каталог пуст.'),
        for (final g in catalog)
          Card(
            child: ListTile(
              title: Text('${g['name']} · ${g['topic']}'),
              subtitle: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text('${g['about'] ?? ''}'),
                  if (g['requested'] == true)
                    const UiText('Просьба отправлена')
                  else
                    button(
                      g['kind'] == 'open'
                          ? 'Вступить'
                          : 'Попросить приглашение',
                      () => join(entry: g),
                    ),
                ],
              ),
            ),
          ),
      ],
      if (tab == 'Управление узла') ...[
        const UiText(
          'Метаданные всех групп узла. Содержание доступно только участникам.',
        ),
        for (final g in admin)
          Card(
            child: ListTile(
              title: Text(
                '${g['name']} · ${g['members'] is List ? (g['members'] as List).length : g['members']} участников',
              ),
              subtitle: button('Удалить группу ${g['name']}', () async {
                final id = '${g['id']}';
                repo.path(id, 'info');
                final value = await form('Удалить группу целиком', [
                  'Пароль входа узла',
                ], secret: true);
                if (value == null) return;
                await mutate(() async {
                  await widget.api.request(
                    '/api/mobile/groups/$id/delete',
                    body: {'password': value[0]},
                  );
                });
              }),
            ),
          ),
      ],
      if (selected != null) conversation(),
    ],
  );
}

class GroupForm extends StatefulWidget {
  final String title;
  final List<String> labels;
  final List<String>? initial;
  final bool secret, creating;
  const GroupForm({
    super.key,
    required this.title,
    required this.labels,
    this.initial,
    this.secret = false,
    this.creating = false,
  });
  @override
  State<GroupForm> createState() => _GroupFormState();
}

class _GroupFormState extends State<GroupForm> {
  late final fields = List.generate(
    widget.labels.length,
    (i) => TextEditingController(text: widget.initial?[i] ?? ''),
  );
  String kind = 'invite';
  String? error;
  @override
  void dispose() {
    for (final c in fields) {
      c.clear();
      c.dispose();
    }
    super.dispose();
  }

  void submit() {
    final values = fields
        .map((f) => widget.secret ? f.text : f.text.trim())
        .toList();
    final settings = widget.creating || widget.title == 'Настройки группы';
    if (settings &&
            (values[0].isEmpty ||
                values[0].runes.length > 64 ||
                values[1].isEmpty ||
                values[1].runes.length > 40 ||
                values[2].runes.length > 300) ||
        !settings && values.any((s) => s.isEmpty)) {
      setState(
        () => error = settings
            ? 'Название 1–64, тематика 1–40, описание до 300 символов.'
            : 'Заполните поля.',
      );
      return;
    }
    Navigator.pop(context, [...values, if (widget.creating) kind]);
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.title),
    content: SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (widget.secret)
            const UiText(
              'Будут уничтожены журнал и файлы группы для всего узла.',
            ),
          for (var i = 0; i < fields.length; i++)
            TextField(
              controller: fields[i],
              obscureText: widget.secret,
              autocorrect: !widget.secret,
              enableSuggestions: !widget.secret,
              decoration: InputDecoration(labelText: widget.labels[i]),
            ),
          if (widget.creating)
            DropdownButton<String>(
              value: kind,
              items: const [
                DropdownMenuItem(
                  value: 'invite',
                  child: Text('По приглашению'),
                ),
                DropdownMenuItem(value: 'open', child: Text('Открытая')),
              ],
              onChanged: (v) => setState(() => kind = v!),
            ),
          if (error != null) Text(error!),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: submit,
        child: Text(widget.secret ? 'Удалить навсегда' : 'Подтвердить'),
      ),
    ],
  );
}

class GroupConfirm extends StatefulWidget {
  final String title;
  final String? detail;
  final int countdown;
  const GroupConfirm({
    super.key,
    required this.title,
    this.detail,
    this.countdown = 0,
  });
  @override
  State<GroupConfirm> createState() => _GroupConfirmState();
}

class _GroupConfirmState extends State<GroupConfirm> {
  late int seconds = widget.countdown;
  Timer? timer;
  File? picked;
  int sentBytes = 0, fileSize = 0;
  bool choosing = false;
  @override
  void initState() {
    super.initState();
    if (seconds > 0)
      timer = Timer.periodic(const Duration(seconds: 1), (_) {
        if (mounted) setState(() => seconds--);
        if (seconds <= 0) timer?.cancel();
      });
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.title),
    content: Text(widget.detail ?? 'Подтвердите действие.'),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context, false),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: seconds > 0 ? null : () => Navigator.pop(context, true),
        child: Text(seconds > 0 ? 'Подождите $seconds с' : 'Подтвердить'),
      ),
    ],
  );
}

class GroupRules extends StatefulWidget {
  final List<int> initial;
  const GroupRules({super.key, required this.initial});
  @override
  State<GroupRules> createState() => _GroupRulesState();
}

class _GroupRulesState extends State<GroupRules> {
  late final deny = widget.initial.toSet();
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const UiText('Правила группы'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final e in {
          2: 'Изображения',
          3: 'Файлы',
          4: 'Аудио',
          5: 'Видео',
          6: 'Ссылки',
        }.entries)
          CheckboxListTile(
            title: Text('Запретить ${e.value}'),
            value: deny.contains(e.key),
            onChanged: (v) => setState(() {
              if (v == true) {
                deny.add(e.key);
              } else {
                deny.remove(e.key);
              }
            }),
          ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(context, deny.toList()..sort()),
        child: const UiText('Сохранить правила'),
      ),
    ],
  );
}
