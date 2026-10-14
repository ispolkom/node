import 'message_actions.dart';
import 'member_badge.dart';
import 'theme.dart';
import 'content_policy.dart';
import 'preferences.dart';
import 'dart:io';
import 'dart:typed_data';
import 'attachments.dart';
import 'platform_files.dart';
import 'voice.dart';
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
  final void Function(String)? onOpened;
  final bool Function() isActive;
  final void Function(ApiFailure) onFailure;
  final VoidCallback onLocked;
  const GroupsFeature({
    super.key,
    required this.api,
    this.onOpened,
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
    voice.onLimit = () => finishVoice(send: true);
    voice.addListener(() {
      if (mounted) setState(() {});
    });
    refresh();
    timer = Timer.periodic(const Duration(seconds: 4), (_) => refresh());
  }

  @override
  void dispose() {
    revision++;
    picked = null;
    PlatformFiles.cancel().catchError((Object _) {});
    timer?.cancel();
    flashTimer?.cancel();
    voice.dispose();
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
    if (id != null) widget.onOpened?.call(id);
    if (selected != null) drafts[selected!] = composer.text;
    revision++;
    setState(() {
      picked = null;
      replyTo = null;
      sentBytes = fileSize = 0;
      selected = id;
      tab = nextTab ?? tab;
      composer.text = drafts[id] ?? '';
      feed = info = null;
      filesView = false;
      section = null;
      picked_.clear();
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
    final quote = replyTo;
    if (findLink(text) != null) {
      setState(() {
        composer.text = removeLinks(text);
        error = 'Ссылки в YANDI не передаются: ссылка удалена из сообщения. Проверьте текст и отправьте снова.';
      });
      return;
    }
    await mutate(
      () => repo.send(id, text, reply: quote),
      success: () {
        composer.clear();
        drafts.remove(id);
        replyTo = null;
      },
    );
  }

  bool get canSend =>
      feed != null &&
      feed!['has_key'] == true &&
      feed!['problem'] != true &&
      feed!['damaged'] != true &&
      group['muted'] != true;
  Widget button(String label, VoidCallback callback, {bool primary = false}) =>
      TextButton(
        style: primary
            ? TextButton.styleFrom(
                textStyle: const TextStyle(
                  fontWeight: FontWeight.bold,
                  decoration: TextDecoration.underline,
                ),
              )
            : null,
        onPressed: busy || loading ? null : callback,
        child: UiText(label),
      );
  String? settingsFor;
  Map<String, dynamic>? replyTo;
  final hiddenAuthors = <String, Set<String>>{};
  // Раздел-фильтр ленты, как в мобильном чате: вид записи или null — все.
  int? section;
  bool filesView = false;
  late final voice = VoiceRecording(voiceBackendFactory(), () => voicePathFactory());
  // выбор записей (по номерам), подсветка записи после перехода по карточке-ответу
  final picked_ = <String>{};
  String? flash;
  Timer? flashTimer;
  // кому уже пересылали запись в этом сеансе: не больше 20 получателей на запись
  final forwardedTo = <String, Set<String>>{};
  static const sections = <(int?, String)>[
    (null, 'Все'),
    (2, 'Картинки'),
    (3, 'Файлы'),
    (4, 'Аудио'),
    (5, 'Видео'),
    (6, 'Ссылки'),
  ];
  final anchors = <String, GlobalKey>{};
  Widget groupItem(Map<String, dynamic> g) {
    final id = '${g['id']}', current = selected == id;
    final members = g['members'] is List
        ? (g['members'] as List).length
        : g['members'];
    final kind = g['kind'] == 'open' ? 'открытая' : 'по приглашению';
    final small = const TextStyle(fontSize: 12, color: Icq.dark);
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: TextButton(
        style: TextButton.styleFrom(
          backgroundColor: Icq.light,
          foregroundColor: Icq.text,
          alignment: Alignment.centerLeft,
          padding: const EdgeInsets.all(10),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(2),
            side: BorderSide(
              color: current ? Icq.header : Icq.border,
              width: current ? 2 : 1,
            ),
          ),
        ),
        onPressed: busy || loading ? null : () => choose(id),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '${g['name']} · ${stateLabel(context, g['role'])}',
              style: const TextStyle(fontWeight: FontWeight.bold),
            ),
            if ('${g['topic'] ?? ''}'.isNotEmpty)
              Text('${tr(context, 'Тематика')}: ${g['topic']}', style: small),
            if ('${g['about'] ?? ''}'.isNotEmpty)
              Text('${g['about']}', style: small),
            Text(
              [
                tr(context, kind),
                if (members != null) '$members ${tr(context, 'уч.')}',
                if (g['muted'] == true) tr(context, 'заглушены'),
                if ((g['requests'] ?? 0) is num && (g['requests'] ?? 0) > 0)
                  '${tr(context, 'просятся')}: ${g['requests']}',
              ].join(' · '),
              style: small,
            ),
          ],
        ),
      ),
    );
  }

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
                UiText(
                  '${'${m['nick'] ?? ''}'.isNotEmpty ? m['nick'] : m['name']} · ${stateLabel(context, m['role'])}${m['muted'] == true ? tr(context, ' · писать запрещено') : ''}',
                ),
                SelectableText('${m['alias']}'),
                Wrap(
                  children: [
                    if (GroupRepository.canTouch(role, '${m['role']}')) ...[
                      for (final action in [
                        m['muted'] == true ? 'unmute' : 'mute',
                        'kick',
                        'ban',
                        // роли — только по матрице: владелец назначает и снимает админов,
                        // админ — модераторов; админ админа не назначает, модератор не назначает никого
                        if (manager && m['role'] == 'member') 'promote',
                        if (manager && m['role'] == 'moderator') 'demote',
                        if (role == 'owner' && m['role'] == 'admin') 'demote',
                        if (role == 'owner' &&
                            (m['role'] == 'member' || m['role'] == 'moderator'))
                          'make_admin',
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
            title: UiText('Просьба: ${r['name']}'),
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
      ],
    ],
  );
  Future<void> pickFile() async {
    final rev = revision;
    choosing = true;
    try {
      final path = await PlatformFiles.choose();
      final bad = path == null ? null : await fileVerdict(path);
      if (bad != null) {
        if (active) setState(() => error = bad);
      } else if (active && rev == revision && path != null)
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

  Future<void> startVoice() async {
    if (busy || !canSend || selected == null) return;
    final why = await voice.start();
    if (why != null && mounted) setState(() => error = why);
  }

  /// Завершить запись: отправить в группу как файл «Голосовое_…» (как в телефоне) или отбросить; файл затирается.
  Future<void> finishVoice({required bool send}) async {
    final (path, secs) = await voice.stop();
    if (path == null) return;
    final f = File(path);
    try {
      if (send && secs >= 1 && active && selected != null) {
        setState(() => picked = f);
        await sendFile();
      }
    } finally {
      if (mounted && picked?.path == f.path) setState(() => picked = null);
      await wipeVoice(f);
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

  /// Цепочка ответов от первого сообщения до записи: у каждой карточки якорь на то, на что отвечали.
  List<({String? seq, String author, String text, bool known})> chainOf(
    Map<String, dynamic> m,
  ) {
    final bySeq = {for (final x in rows(feed, 'messages')) '${x['seq']}': x};
    final out = <({String? seq, String author, String text, bool known})>[];
    final seen = <String>{};
    Map<String, dynamic>? cur = m;
    while (cur != null && seen.add('${cur['seq']}')) {
      out.add((
        seq: '${cur['seq']}',
        author: '${cur['author']}',
        text:
            '${cur['text'] ?? (cur['attachment'] is Map ? (cur['attachment'] as Map)['filename'] : '')}',
        known: true,
      ));
      final r = cur['reply'];
      if (r is! Map) break;
      final parent = bySeq['${r['seq']}'];
      if (parent == null) {
        out.add((
          seq: null,
          author: '${r['author'] ?? ''}',
          text: '${r['text'] ?? ''}',
          known: false,
        ));
        break;
      }
      cur = parent;
    }
    return out.reversed.toList();
  }

  void jumpTo(String seq) {
    final ctx = anchors['$selected:$seq']?.currentContext;
    setState(() => flash = seq);
    flashTimer?.cancel();
    flashTimer = Timer(const Duration(milliseconds: 1800), () {
      if (mounted && flash == seq) setState(() => flash = null);
    });
    if (ctx != null) {
      Scrollable.ensureVisible(
        ctx,
        duration: const Duration(milliseconds: 200),
      );
    }
  }

  Future<void> showThread(Map<String, dynamic> m) async {
    final chain = chainOf(m);
    final seq = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const UiText('Цепочка ответов: сверху — с чего всё началось'),
        content: SizedBox(
          width: 420,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (var i = 0; i < chain.length; i++)
                  Padding(
                    padding: EdgeInsets.only(
                      left: (i * 12.0).clamp(0, 72),
                      bottom: 6,
                    ),
                    child: InkWell(
                      key: ValueKey('thread-$i'),
                      onTap: chain[i].seq == null
                          ? null
                          : () => Navigator.pop(ctx, chain[i].seq),
                      child: Container(
                        padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
                        decoration: BoxDecoration(
                          color: Icq.light,
                          border: Border(
                            left: BorderSide(
                              color: i == chain.length - 1
                                  ? Icq.header
                                  : Icq.peer,
                              width: 3,
                            ),
                          ),
                        ),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '${chain[i].author}${chain[i].known ? '' : ' · сообщение недоступно'}',
                              style: const TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.bold,
                                color: Icq.peer,
                              ),
                            ),
                            Text(chain[i].text),
                          ],
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const UiText('Закрыть'),
          ),
        ],
      ),
    );
    if (seq != null && active) jumpTo(seq);
  }

  /// Переслать записи группы выбранным контактам (до 5 за раз, до 20 на запись за сеанс); файл пересылается как файл.
  Future<void> forward(List<Map<String, dynamic>> items) async {
    if (items.isEmpty || busy || selected == null) return;
    final id = selected!;
    final shell = ShellRepository(widget.api);
    List<Map<String, dynamic>> contacts = [], trusted = [];
    try {
      contacts = await shell.contacts();
      trusted = ShellRepository.list(await shell.trusted(), 'peers');
    } catch (e) {
      failure(e);
      return;
    }
    String peerOf(Map<String, dynamic> c) {
      final short = '${c['short_id']}';
      for (final t in trusted) {
        final tid = t['id'];
        if (tid is String && tid.startsWith(short)) return tid;
      }
      return short;
    }

    if (!mounted) return;
    final to = await pickRecipients(
      context,
      title: items.length == 1
          ? 'Переслать запись'
          : 'Переслать записи (${items.length})',
      contacts: contacts,
    );
    if (to == null || to.isEmpty || !active) return;
    final targets = [for (final c in to) peerOf(c)];
    for (final m in items) {
      final all = {...?forwardedTo['$id:${m['seq']}'], ...targets};
      if (all.length > 20) {
        setState(
          () => error =
              'Одну запись можно переслать не больше чем 20 получателям.',
        );
        return;
      }
    }
    final files = FileRepository(widget.api);
    await mutate(() async {
      for (final m in items) {
        final att = m['attachment'] is Map
            ? Map<String, dynamic>.from(m['attachment'] as Map)
            : null;
        final text = '${m['text'] ?? ''}';
        Uint8List? bytes;
        try {
          if (att != null) {
            final size = att['size'];
            if (size is! num || size <= 0) throw const ApiFailure('protocol');
            bytes = await files.binary.bytes(
              repo.path(id, 'file/${m['seq']}'),
              limit: size.toInt(),
            );
          }
          for (var i = 0; i < to.length; i++) {
            if (!active) return;
            if (bytes != null) {
              await files.uploadBytes(
                targets[i],
                '${att!['filename']}',
                bytes,
                phone: to[i]['is_phone'] == true,
                isActive: () => active,
              );
            } else if (text.isNotEmpty) {
              await shell.send(targets[i], text);
            }
            (forwardedTo['$id:${m['seq']}'] ??= <String>{}).add(targets[i]);
          }
        } finally {
          bytes?.fillRange(0, bytes.length, 0);
        }
      }
    }, success: () => picked_.clear());
  }

  Future<void> deletePicked() async {
    final id = selected!;
    final items = [
      for (final m in rows(feed, 'messages'))
        if (picked_.contains('${m['seq']}') && canDelete(m)) m,
    ];
    if (items.isEmpty) return;
    if (!await confirm(
      'Удалить выбранные записи (${items.length})?',
      detail: 'Записи будут удалены для участников группы.',
    ))
      return;
    await mutate(() async {
      for (final m in items) {
        await repo.act(id, 'mod', {'action': 'delete_entry', 'seq': m['seq']});
      }
    }, success: () => picked_.clear());
  }

  Future<void> messageMenu(Map<String, dynamic> m, Offset at) async {
    if (busy || selected == null) return;
    final id = selected!;
    final own = m['mine'] == true;
    final authorRole = rows(info, 'members')
        .where((x) => x['alias'] == m['from'])
        .map((x) => '${x['role']}')
        .firstOrNull;
    final commands = [
      commandReply,
      if (!own && m['seq'] != null) commandApprove,
      if (!own && m['seq'] != null) commandDisapprove,
      if (m['reply'] is Map) commandThread,
      if ('${m['text'] ?? ''}'.isNotEmpty) commandCopy,
      if ('${m['text'] ?? ''}'.isNotEmpty || m['attachment'] is Map)
        commandShare,
      commandSelect,
      if (!own) commandHideAuthor,
      if (canDelete(m)) commandDelete,
      if (!own &&
          authorRole != null &&
          GroupRepository.canTouch(role, authorRole))
        commandMute,
    ];
    final choice = await showMessageMenu(context, at, commands);
    if (!mounted || choice == null) return;
    switch (choice) {
      case commandApprove:
      case commandDisapprove:
        final v = choice == commandApprove ? 1 : -1;
        await mutate(() async {
          await repo.act(id, 'vote', {'seq': m['seq'], 'v': v});
        });
      case commandReply:
        setState(() {
          replyTo = {
            'seq': m['seq'],
            'author': own ? tr(context, 'Вы') : '${m['author']}',
            'text': quoteExcerpt(
              '${m['text'] ?? ''}'.isEmpty
                  ? (m['attachment'] is Map
                        ? (m['attachment'] as Map)['filename']
                        : '')
                  : m['text'],
            ),
          };
        });
      case commandCopy:
        await Clipboard.setData(ClipboardData(text: '${m['text'] ?? ''}'));
      case commandThread:
        await showThread(m);
      case commandShare:
        await forward([m]);
      case commandSelect:
        setState(() => picked_.add('${m['seq']}'));
      case commandMute:
        await mod('mute', {'alias': m['from'], 'name': m['author']});
      case commandHideAuthor:
        setState(() {
          (hiddenAuthors[id] ??= <String>{}).add('${m['from']}');
        });
      case commandDelete:
        await deleteMessage(m);
    }
  }

  Widget bubble(Map<String, dynamic> m) {
    final own = m['mine'] == true;
    final ts = DateTime.fromMillisecondsSinceEpoch(
      (m['ts'] as num).toInt(),
    ).toLocal();
    final hm =
        '${ts.hour.toString().padLeft(2, '0')}:${ts.minute.toString().padLeft(2, '0')}';
    final reply = m['reply'] is Map ? m['reply'] as Map : null;
    var tapAt = Offset.zero;
    return Align(
      key: anchors.putIfAbsent('${selected}:${m['seq']}', () => GlobalKey()),
      alignment: own ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: (d) => tapAt = d.globalPosition,
          onTap: () {
            if (picked_.isEmpty) {
              messageMenu(m, tapAt);
            } else {
              setState(() {
                final k = '${m['seq']}';
                if (!picked_.remove(k)) picked_.add(k);
              });
            }
          },
          child: Container(
            margin: const EdgeInsets.symmetric(vertical: 4),
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 4),
            decoration: BoxDecoration(
              color: own ? Icq.sent : Colors.white,
              border: Border.all(
                color: picked_.contains('${m['seq']}')
                    ? const Color(0xFF2E86DE)
                    : (flash == '${m['seq']}'
                          ? Colors.black
                          : (own ? Icq.mine : Icq.peer)),
                width: picked_.contains('${m['seq']}') || flash == '${m['seq']}'
                    ? 3
                    : 1.5,
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                if (!own)
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      MemberBadge(
                        api: widget.api,
                        gid: '$selected',
                        alias: '${m['from']}',
                        name: '${m['author']}',
                      ),
                      const SizedBox(width: 6),
                      Text(
                        '${m['author']}',
                        style: const TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.bold,
                          color: Icq.peer,
                        ),
                      ),
                    ],
                  ),
                if (reply != null)
                  QuoteCard(
                    author: '${reply['author']}',
                    text: '${reply['text'] ?? ''}',
                    onTap: () => jumpTo('${reply['seq']}'),
                  ),
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
                  Text(
                    m['text'] == null
                        ? tr(context, 'Вложение (недоступно)')
                        : stripLinks('${m['text']}'),
                  ),
                Align(
                  alignment: Alignment.centerRight,
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (m['coins'] is num) ...[
                        AnimatedSwitcher(
                          duration: const Duration(milliseconds: 450),
                          transitionBuilder: (child, animation) => ScaleTransition(
                            scale: animation,
                            child: FadeTransition(opacity: animation, child: child),
                          ),
                          child: Text(
                            '🪙 ${(m['coins'] as num) > 0 ? '+' : ''}${m['coins']}',
                            key: ValueKey('coins-${m['seq']}-${m['coins']}'),
                            style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.bold,
                              color: (m['coins'] as num) < 0
                                  ? const Color(0xFFB00020)
                                  : Icq.dark,
                            ),
                          ),
                        ),
                        const SizedBox(width: 10),
                      ],
                      Text(
                        hm,
                        style: const TextStyle(fontSize: 11, color: Icq.dark),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget requestCard(Map<String, dynamic> r) => Container(
    margin: const EdgeInsets.symmetric(vertical: 4),
    padding: const EdgeInsets.all(10),
    decoration: BoxDecoration(
      color: Icq.peer.withValues(alpha: 0.10),
      border: Border.all(color: Icq.peer.withValues(alpha: 0.7)),
    ),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.how_to_reg_outlined, size: 18, color: Icq.peer),
            const SizedBox(width: 6),
            Flexible(
              child: Text(
                '${r['name'] ?? tr(context, 'Новая просьба')}',
                style: const TextStyle(
                  fontWeight: FontWeight.bold,
                  color: Icq.peer,
                ),
              ),
            ),
          ],
        ),
        const UiText('просит добавить его в группу или прислать приглашение'),
        const UiText(
          'служебное: видят владелец и модераторы',
          style: TextStyle(fontSize: 11, color: Icq.dark),
        ),
        Wrap(
          spacing: 14,
          children: [
            for (final answer in [
              'Пригласить',
              'Пропустить',
              'Больше не напоминать',
            ])
              CommandText(
                answer,
                onTap: busy
                    ? null
                    : () async {
                        final id = selected!;
                        await mutate(() async {
                          await repo.act(id, 'answer', {
                            'rid': r['id'],
                            'accept': answer == 'Пригласить',
                            'never': answer == 'Больше не напоминать',
                          });
                        });
                      },
              ),
          ],
        ),
      ],
    ),
  );

  Widget conversation({bool wide = false}) {
    final history = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: 10,
          runSpacing: 4,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            Text(
              '${group['name'] ?? tr(context, 'Загрузка группы…')}',
              style: const TextStyle(fontSize: 20, fontWeight: FontWeight.bold),
            ),
            if (group['kind'] != null)
              UiText(
                group['kind'] == 'open' ? 'открытая' : 'по приглашению',
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
            if (group['my_alias'] != null)
              UiText(
                '${tr(context, 'вы')}: ${group['my_alias']} · ${stateLabel(context, group['role'])}',
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
            if (info != null)
              button(
                settingsFor == selected
                    ? 'Закрыть настройки'
                    : 'Настройки группы',
                () => setState(
                  () => settingsFor = settingsFor == selected ? null : selected,
                ),
              ),
          ],
        ),
        const UiText(
          'История не старше календарного года.',
          style: TextStyle(fontSize: 12, color: Icq.dark),
        ),
        const SizedBox(height: 8),
        if (feed?['damaged'] == true)
          const UiText('Повреждена лента на диске.'),
        if (feed?['problem'] == true)
          const UiText('Ключ владельца изменился: доверие не подтверждено.'),
        if (feed != null && feed!['has_key'] != true)
          const UiText('Ждём ключ группы.'),
        if (group['muted'] == true) const UiText('Вам запрещено писать.'),
        if (feed?['deny'] is List && (feed!['deny'] as List).isNotEmpty)
          UiText(
            'Запрещено: ${(feed!['deny'] as List).map((k) => const {2: 'изображения', 3: 'файлы', 4: 'аудио', 5: 'видео', 6: 'ссылки'}[k] ?? '$k').join(', ')}',
          ),
        if (feed != null && rows(feed, 'messages').isEmpty)
          const UiText('История пуста.'),
        if ((hiddenAuthors[selected] ?? const <String>{}).isNotEmpty)
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 8,
            children: [
              UiText(
                'Скрыто пользователей: ${hiddenAuthors[selected]!.length}',
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
              const SizedBox(width: 8),
              CommandText(
                'Показать скрытых',
                onTap: () => setState(() => hiddenAuthors.remove(selected)),
              ),
            ],
          ),
        if (picked_.isNotEmpty)
          Wrap(
            spacing: 14,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              UiText(
                'Выбрано: ${picked_.length}',
                style: const TextStyle(fontWeight: FontWeight.bold),
              ),
              CommandText(
                'Переслать выбранные',
                onTap: busy
                    ? null
                    : () => forward([
                        for (final m in rows(feed, 'messages'))
                          if (picked_.contains('${m['seq']}')) m,
                      ]),
              ),
              if (rows(
                feed,
                'messages',
              ).any((m) => picked_.contains('${m['seq']}') && canDelete(m)))
                CommandText(
                  'Удалить выбранные',
                  onTap: busy ? null : deletePicked,
                ),
              CommandText('Снять выбор', onTap: () => setState(picked_.clear)),
            ],
          ),
        // «Файлы группы» — отдельный вид без сообщений: картинки, файлы, аудио, видео и ссылки по разделам.
        Wrap(
          spacing: 14,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            CommandText(
              filesView ? 'Назад к сообщениям' : 'Файлы группы',
              onTap: () => setState(() {
                filesView = !filesView;
                section = null;
              }),
            ),
            if (filesView)
              for (final (kind, label) in sections)
                InkWell(
                  onTap: () => setState(() => section = kind),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 2),
                    child: UiText(
                      label,
                      style: TextStyle(
                        fontSize: 13,
                        fontWeight: section == kind
                            ? FontWeight.bold
                            : FontWeight.normal,
                        color: section == kind ? Icq.active : Icq.header,
                        decoration: section == kind
                            ? TextDecoration.none
                            : TextDecoration.underline,
                      ),
                    ),
                  ),
                ),
          ],
        ),
        // Просьбы вступить — служебные карточки в ленте, видны управляющим.
        if (staff && settingsFor != selected)
          for (final r in rows(info, 'requests')) requestCard(r),
        if (filesView &&
            !rows(feed, 'messages').any(
              (m) =>
                  m['kind'] is int &&
                  (m['kind'] as int) >= 2 &&
                  (section == null || m['kind'] == section),
            ))
          const UiText('В этом разделе пусто'),
        for (final m in rows(feed, 'messages'))
          if (!(hiddenAuthors[selected] ?? const <String>{}).contains(
                '${m['from']}',
              ) &&
              (!filesView ||
                  (m['kind'] is int &&
                      (m['kind'] as int) >= 2 &&
                      (section == null || m['kind'] == section))))
            bubble(m),
        if (info != null && settingsFor == selected) ...[
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
            if (role == 'owner')
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 6),
                child: Wrap(
                  crossAxisAlignment: WrapCrossAlignment.center,
                  spacing: 14,
                  children: [
                    const UiText(
                      'Автоочистка неактивных (рядовые без записей):',
                      style: TextStyle(fontWeight: FontWeight.bold),
                    ),
                    for (final days in const [0, 30, 60, 90])
                      InkWell(
                        key: ValueKey('autoclean-$days'),
                        onTap: busy
                            ? null
                            : () => mutate(() async {
                                await repo.act(selected!, 'autoclean', {
                                  'days': days,
                                });
                              }),
                        child: UiText(
                          days == 0 ? 'Выключена' : '$days дней',
                          style: TextStyle(
                            fontWeight: (group['auto_clean_days'] ?? 0) == days
                                ? FontWeight.bold
                                : FontWeight.normal,
                            color: (group['auto_clean_days'] ?? 0) == days
                                ? Icq.active
                                : Icq.header,
                            decoration: (group['auto_clean_days'] ?? 0) == days
                                ? TextDecoration.none
                                : TextDecoration.underline,
                          ),
                        ),
                      ),
                  ],
                ),
              )
            else if ((group['auto_clean_days'] ?? 0) is int &&
                (group['auto_clean_days'] ?? 0) > 0)
              UiText(
                'В группе включена автоочистка: кто не пишет ${group['auto_clean_days']} дней, исключается.',
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
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
          () {
            final mine = rows(info, 'members').where(
              (m) => m['alias'] == group['my_alias'],
            );
            final shared =
                mine.isNotEmpty && '${mine.first['nick'] ?? ''}'.isNotEmpty;
            return Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Wrap(
                crossAxisAlignment: WrapCrossAlignment.center,
                spacing: 14,
                children: [
                  UiText(
                    shared
                        ? 'Участники видят вас по нику и аватарке.'
                        : 'Участники видят вас только по псевдониму ${group['my_alias']}.',
                    style: const TextStyle(fontWeight: FontWeight.bold),
                  ),
                  InkWell(
                    key: const ValueKey('card-share'),
                    onTap: busy
                        ? null
                        : () => mutate(() async {
                            await repo.act(selected!, 'profile', {
                              'share': !shared,
                            });
                          }),
                    child: UiText(
                      shared
                          ? 'Показывать только псевдоним'
                          : 'Показывать мой ник и аватар',
                      style: const TextStyle(
                        color: Icq.header,
                        decoration: TextDecoration.underline,
                      ),
                    ),
                  ),
                ],
              ),
            );
          }(),
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
    final field = Focus(
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
        decoration: InputDecoration(
          hintText: tr(context, 'Сообщение группе'),
          helperText: tr(context, 'Ctrl+Enter — отправить'),
        ),
      ),
    );
    final sendButton = SendOrMic(
      controller: composer,
      sendTip: 'Отправить группе',
      hasFile: picked != null,
      onSend: !busy && canSend
          ? (picked != null && composer.text.trim().isEmpty ? sendFile : send)
          : null,
      onMic: !busy && canSend ? startVoice : null,
    );
    final attach = ComposerTools(
      onAttach: busy || choosing || !canSend ? null : pickFile,
      controller: composer,
    );
    final input = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (replyTo != null)
          QuoteCard(
            author: '${replyTo!['author']}',
            text: '${replyTo!['text']}',
            trailing: CommandText(
              'Отмена',
              onTap: () => setState(() => replyTo = null),
            ),
          ),
        if (voice.active)
          VoiceBar(
            seconds: voice.seconds,
            onCancel: () => finishVoice(send: false),
            onSend: () => finishVoice(send: true),
          )
        else
          LayoutBuilder(
            builder: (context, box) => box.maxWidth < 520
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      field,
                      Wrap(children: [attach, sendButton]),
                    ],
                  )
                : Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      attach,
                      const SizedBox(width: 6),
                      Expanded(child: field),
                      const SizedBox(width: 6),
                      sendButton,
                    ],
                  ),
          ),
        if (picked != null)
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
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
          ),
        if (fileSize > 0) ...[
          LinearProgressIndicator(value: sentBytes / fileSize),
          UiText(
            'Передано демону: $sentBytes / $fileSize байт. Доставка подтверждается отдельно.',
          ),
        ],
      ],
    );
    if (!wide)
      return Panel(
        color: Colors.white,
        child: Column(children: [history, input]),
      );
    return Container(
      decoration: const BoxDecoration(color: Colors.white, border: Icq.frame),
      child: Column(
        children: [
          Expanded(
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(12),
              child: history,
            ),
          ),
          Padding(padding: const EdgeInsets.all(10), child: input),
        ],
      ),
    );
  }

  Widget groupList() => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Wrap(
        spacing: 6,
        runSpacing: 6,
        children: [
          button('Создать группу', create, primary: true),
          button('Вступить по приглашению', () => join()),
          button('Обновить группы', refresh),
        ],
      ),
      const SizedBox(height: 10),
      if (tab == 'Свои группы') ...[
        if (!loading && mine.isEmpty)
          const UiText('Вы ещё не состоите в группах.'),
        for (final g in mine) groupItem(g),
        for (final g in gone)
          ListTile(
            title: UiText('${g['name']}: удалена или вы исключены'),
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
              title: UiText(
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
    ],
  );

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      if (loading || busy) const LinearProgressIndicator(),
      if (error != null) NoticePanel(error!, error: true),
      Panel(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const UiText(
              'Группы. Группы телефонов со сквозным шифрованием: компьютер участвует как обычное устройство. Содержимое доступно только участникам и хранится на узле в зашифрованном виде.',
            ),
            const SizedBox(height: 10),
            TabStrip(
              labels: const ['Мои группы', 'Список групп', 'Управление узла'],
              selected: tab == 'Свои группы'
                  ? 'Мои группы'
                  : tab == 'Каталог'
                  ? 'Список групп'
                  : tab,
              onSelected: busy
                  ? null
                  : (label) => choose(
                      null,
                      nextTab: label == 'Мои группы'
                          ? 'Свои группы'
                          : label == 'Список групп'
                          ? 'Каталог'
                          : label,
                    ),
            ),
          ],
        ),
      ),
      const SizedBox(height: 12),
      LayoutBuilder(
        builder: (context, size) {
          final list = Panel(
            title: 'Группы',
            padding: const EdgeInsets.all(10),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [groupList()],
            ),
          );
          final wideList = Container(
            decoration: const BoxDecoration(
              color: Icq.panel,
              border: Icq.frame,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                const PageHeader('Группы'),
                Expanded(
                  child: SingleChildScrollView(
                    padding: const EdgeInsets.all(10),
                    child: groupList(),
                  ),
                ),
              ],
            ),
          );
          final empty = Panel(
            color: Colors.white,
            child: SizedBox(
              height: size.maxWidth < 700
                  ? (MediaQuery.sizeOf(context).height - 280).clamp(
                          350.0,
                          800.0,
                        ) -
                        34
                  : (MediaQuery.sizeOf(context).height - 280).clamp(
                          350.0,
                          800.0,
                        ) -
                        34,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const UiText(
                    'Выберите группу слева.',
                    style: TextStyle(color: Icq.dark),
                  ),
                  const Spacer(),
                  if (tab == 'Свои группы')
                    TextField(
                      enabled: false,
                      decoration: InputDecoration(
                        hintText: tr(context, 'Сообщение группе'),
                      ),
                    ),
                  const SizedBox(height: 8),
                  if (tab == 'Свои группы')
                    const FilledButton(
                      onPressed: null,
                      child: UiText('Отправить группе'),
                    ),
                ],
              ),
            ),
          );
          if (size.maxWidth < 700)
            return Column(
              children: [list, selected == null ? empty : conversation()],
            );
          return SizedBox(
            height: (MediaQuery.sizeOf(context).height - 280).clamp(
              350.0,
              800.0,
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(
                  width: size.maxWidth < 1000 ? 260 : 320,
                  child: wideList,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: selected == null ? empty : conversation(wide: true),
                ),
              ],
            ),
          );
        },
      ),
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
    title: UiText(widget.title),
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
              decoration: InputDecoration(
                labelText: tr(context, widget.labels[i]),
              ),
            ),
          if (widget.creating)
            DropdownButton<String>(
              value: kind,
              items: const [
                DropdownMenuItem(
                  value: 'invite',
                  child: UiText('По приглашению'),
                ),
                DropdownMenuItem(value: 'open', child: UiText('Открытая')),
              ],
              onChanged: (v) => setState(() => kind = v!),
            ),
          if (error != null) UiText(error!),
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
        child: UiText(widget.secret ? 'Удалить навсегда' : 'Подтвердить'),
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
    title: UiText(widget.title),
    content: UiText(widget.detail ?? 'Подтвердите действие.'),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context, false),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: seconds > 0 ? null : () => Navigator.pop(context, true),
        child: UiText(seconds > 0 ? 'Подождите $seconds с' : 'Подтвердить'),
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
            title: UiText('Запретить ${e.value}'),
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
