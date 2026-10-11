import 'dart:io';
import 'dart:math';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../models/group_invite.dart';
import '../services/app_state.dart';
import '../services/file_transfer_service.dart';
import '../services/group_files.dart';
import '../services/group_service.dart';
import '../services/vault_files.dart';
import 'file_viewer_screen.dart';
import '../theme.dart';
import '../widgets/secure_keyboard.dart';
import 'group_info_screen.dart';

/// Разделы-фильтры группы: вид записи → подпись.
const groupSections = <(int?, String)>[
  (null, 'Все'),
  (GroupKind.image, 'Картинки'),
  (GroupKind.file, 'Файлы'),
  (GroupKind.audio, 'Аудио'),
  (GroupKind.video, 'Видео'),
  (GroupKind.link, 'Ссылки'),
];

/// Вид записи по тексту: ссылка — отдельный вид, чтобы группа могла её запретить.
int kindOfText(String t) => RegExp(r'(https?://|www\.)\S+', caseSensitive: false).hasMatch(t) ? GroupKind.link : GroupKind.text;

class GroupChatScreen extends StatefulWidget {
  final String gid;
  const GroupChatScreen({super.key, required this.gid});
  @override
  State<GroupChatScreen> createState() => _GroupChatScreenState();
}

class _GroupChatScreenState extends State<GroupChatScreen> {
  final _ctrl = TextEditingController();
  final _focus = FocusNode();
  final _scroll = ScrollController();
  int? _section;
  List<GroupEntry> _entries = const [];
  int _shownCount = -1;
  final Map<String, String> _paths = {};       // номер файла на узле → где лежит у меня
  List<JoinRequest> _requests = const [];     // просьбы вступить (видят владелец и модераторы)
  final Map<String, double> _progress = {};    // номер файла (или 'up') → доля

  @override
  void initState() {
    super.initState();
    _focus.addListener(() => setState(() {}));
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      await context.read<AppState>().syncGroups();
      await _load();
    });
  }

  @override
  void dispose() {
    _ctrl.dispose();
    _focus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  GroupService? get _svc => context.read<AppState>().groups;

  Future<void> _load() async {
    final svc = _svc;
    if (svc == null) return;
    final list = await svc.messages(widget.gid, kind: _section);
    List<JoinRequest> reqs = const [];
    final gi = svc.byId(widget.gid);
    if (gi != null && gi.canModerate && gi.kind == 'invite' && _section == null) {
      try { reqs = await svc.joinRequests(widget.gid); } catch (_) {}
    }
    if (!mounted) return;
    setState(() { _entries = list; _requests = reqs; });
    if (list.length != _shownCount) {
      _shownCount = list.length;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients) _scroll.jumpTo(_scroll.position.maxScrollExtent);
      });
    }
  }

  String _fmtSize(int b) => b >= 1 << 20 ? '${(b / (1 << 20)).toStringAsFixed(1)} МБ' : '${(b / 1024).ceil()} КБ';

  Future<void> _attach() async {
    final svc = _svc!;
    final r = await FilePicker.platform.pickFiles(allowMultiple: false);
    final path = r?.files.single.path;
    if (path == null) return;
    final name = r!.files.single.name;
    final kind = GroupFiles.kindForName(name);
    if (!svc.rules(widget.gid).allows(kind)) { _toast('В этой группе запрещён такой вид файлов'); return; }
    setState(() => _progress['up'] = 0);
    try {
      final d = await GroupFiles(svc.api).upload(widget.gid, File(path), name, onProgress: (done, total) {
        if (mounted) setState(() => _progress['up'] = done / total);
      });
      try {
        await svc.send(widget.gid, kind, d);
      } catch (e) {
        try { await svc.api.fileDelete(widget.gid, d['f'] as String); } catch (_) {}   // запись не ушла — файл на узле не нужен
        rethrow;
      }
      _paths[d['f'] as String] = path;   // свой файл уже у меня
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast('Файл не отправлен: $e');
    } finally {
      if (mounted) setState(() => _progress.remove('up'));
    }
  }

  Future<String?> _localPath(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    final mem = _paths[fid];
    if (mem != null && File(mem).existsSync()) return mem;
    final saved = await _svc!.store.read('grp_dl_$fid');
    if (saved != null && File(saved).existsSync()) { _paths[fid] = saved; return saved; }
    return null;
  }

  Future<void> _openFile(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    var path = await _localPath(d);
    if (path == null) {
      final dir = await FileTransferService.downloadsDir();
      final safe = ((d['n'] as String?) ?? 'file').replaceAll(RegExp(r'[\\/\x00-\x1f]'), '_');
      final prefix = List.generate(4, (_) => Random.secure().nextInt(256).toRadixString(16).padLeft(2, '0')).join();
      final out = File('${dir.path}/${prefix}_$safe');
      setState(() => _progress[fid] = 0);
      try {
        await GroupFiles(_svc!.api).download(widget.gid, d, out, onProgress: (done, total) {
          if (mounted) setState(() => _progress[fid] = done / total);
        });
      } catch (e) {
        _toast('Не скачано: $e');
        return;
      } finally {
        if (mounted) setState(() => _progress.remove(fid));
      }
      await _svc!.store.write('grp_dl_$fid', out.path);
      _paths[fid] = out.path;
      path = out.path;
    }
    final v = await VaultFiles.describe(path);
    if (!mounted) return;
    await Navigator.push(context, MaterialPageRoute(builder: (_) => FileViewerScreen(file: v)));
  }

  Future<void> _send() async {
    final text = _ctrl.text.trim();
    if (text.isEmpty) return;
    final svc = _svc!;
    try {
      await svc.send(widget.gid, kindOfText(text), {'t': text});
      _ctrl.clear();
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast('Не отправлено: $e');
    }
  }

  void _toast(String m) => ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(m)));

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();   // перерисовка, когда пришли новые записи
    final svc = state.groups;
    final g = svc?.byId(widget.gid);
    // новые записи пришли по намёку — подтянуть
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted || svc == null) return;
      final list = await svc.messages(widget.gid, kind: _section);
      if (mounted && (list.length != _entries.length || (list.isNotEmpty && _entries.isNotEmpty && list.last.seq != _entries.last.seq))) _load();
    });
    if (g == null || svc == null) {
      return Scaffold(appBar: AppBar(title: const Text('Группа')), body: const Center(child: Text('Группа недоступна', style: TextStyle(color: AppTheme.textSecondary))));
    }
    final noKey = !state.groups!.hasProblem(g.id) && g.epoch == 0;
    return Scaffold(
      appBar: AppBar(
        title: InkWell(
          onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupInfoScreen(gid: g.id))),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(g.name, style: const TextStyle(fontSize: 16)),
            Text('${g.members} уч. · вы: ${g.myAlias}', style: const TextStyle(fontSize: 11, color: AppTheme.textSecondary)),
          ]),
        ),
        actions: [IconButton(icon: const Icon(Icons.settings_outlined), tooltip: 'Настройки группы', onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupInfoScreen(gid: g.id))))],
      ),
      body: Column(children: [
        SizedBox(
          height: 44,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: 8),
            children: [
              for (final (k, label) in groupSections)
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
                  child: ChoiceChip(
                    label: Text(label, style: TextStyle(fontSize: 12, color: _section == k ? Colors.black : AppTheme.text)),
                    selected: _section == k,
                    selectedColor: AppTheme.accent,
                    backgroundColor: AppTheme.surface,
                    onSelected: (_) { setState(() => _section = k); _load(); },
                  ),
                ),
            ],
          ),
        ),
        if (svc.hasProblem(g.id))
          Container(
            width: double.infinity, color: AppTheme.incoming.withOpacity(0.15), padding: const EdgeInsets.all(8),
            child: const Text('Ключ владельца группы изменился. Не доверяйте новым ключам, пока не проверите владельца лично.', style: TextStyle(color: AppTheme.incoming, fontSize: 12)),
          )
        else if (noKey)
          Container(
            width: double.infinity, color: AppTheme.surface, padding: const EdgeInsets.all(8),
            child: const Text('Ключ группы ещё не создан владельцем. Сообщения появятся, когда он откроет приложение.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
        Expanded(
          child: _entries.isEmpty && _requests.isEmpty
              ? Center(child: Text(_section == null ? 'Сообщений пока нет' : 'В этом разделе пусто', style: const TextStyle(color: AppTheme.textSecondary)))
              : ListView.builder(
                  controller: _scroll,
                  padding: const EdgeInsets.all(12),
                  itemCount: _entries.length + _requests.length,
                  itemBuilder: (_, i) {
                    if (i >= _entries.length) return _RequestCard(request: _requests[i - _entries.length], onTap: () => _requestMenu(g, _requests[i - _entries.length]));
                    final e = _entries[i];
                    final mine = e.from == g.myAlias;
                    return _GroupBubble(
                      entry: e,
                      mine: mine,
                      author: svc.displayName(g.id, e.from),
                      sizeLabel: e.body['z'] is int ? _fmtSize(e.body['z'] as int) : null,
                      progress: e.body['f'] is String ? _progress[e.body['f']] : null,
                      onOpen: e.body['f'] is String ? () => _openFile(e.body) : null,
                      onLongPress: g.canModerate || mine ? () => _entryMenu(g, e) : null,
                    );
                  },
                ),
        ),
        _input(g, state),
        if (state.secureKeyboard && _focus.hasFocus) SecureKeyboard(controller: _ctrl, onEnter: _send),
      ]),
    );
  }

  Widget _input(GroupInfo g, AppState state) {
    final denied = g.muted;
    return Container(
      color: AppTheme.surface,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
      child: SafeArea(
        top: false,
        child: Row(children: [
          IconButton(
            tooltip: 'Прикрепить файл',
            icon: Icon(Icons.attach_file, color: _progress.containsKey('up') ? AppTheme.accent : AppTheme.textSecondary),
            onPressed: denied || _progress.containsKey('up') ? null : _attach,
          ),
          Expanded(
            child: TextField(
              controller: _ctrl,
              focusNode: _focus,
              enabled: !denied,
              style: const TextStyle(color: AppTheme.text),
              maxLines: null,
              keyboardType: state.secureKeyboard ? SecureKeyboard.inputType : null,
              enableSuggestions: !state.secureKeyboard,
              autocorrect: !state.secureKeyboard,
              enableIMEPersonalizedLearning: !state.secureKeyboard,
              decoration: InputDecoration(
                hintText: denied ? 'Вы заглушены в этой группе' : 'Сообщение…',
                hintStyle: const TextStyle(color: AppTheme.textSecondary),
                filled: true,
                fillColor: AppTheme.bg,
                border: OutlineInputBorder(borderRadius: BorderRadius.circular(24), borderSide: BorderSide.none),
                contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              ),
            ),
          ),
          IconButton(icon: const Icon(Icons.send, color: AppTheme.accent), onPressed: denied ? null : _send),
        ]),
      ),
    );
  }

  /// Просьба вступить, показанная карточкой в чате: добавить (приглашение личным сообщением), игнорировать, больше не напоминать.
  void _requestMenu(GroupInfo g, JoinRequest r) {
    final state = context.read<AppState>();
    Future<void> run(Future<void> Function() act, String done) async {
      try { await act(); _toast(done); } catch (e) { _toast('$e'); }
      await _load();
    }
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          Padding(padding: const EdgeInsets.all(16), child: Text('${r.name.isEmpty ? 'Кто-то' : r.name} просит приглашение в группу', style: const TextStyle(color: AppTheme.text, fontSize: 16))),
          ListTile(
            leading: const Icon(Icons.person_add_alt, color: AppTheme.accent),
            title: const Text('Добавить', style: TextStyle(color: AppTheme.text)),
            subtitle: const Text('Ему уйдёт приглашение личным сообщением', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); run(() async {
              final code = await _svc!.newInvite(g.id, uses: 1, ttlHours: 72);
              await state.sendMessage(r.peerId, GroupInviteMsg.encode(g.id, code, g.name));
              await _svc!.closeRequest(g.id, r.id);
            }, 'Приглашение отправлено'); },
          ),
          ListTile(
            leading: const Icon(Icons.close, color: AppTheme.textSecondary),
            title: const Text('Игнорировать', style: TextStyle(color: AppTheme.text)),
            onTap: () { Navigator.pop(ctx); run(() => _svc!.closeRequest(g.id, r.id), 'Просьба закрыта'); },
          ),
          ListTile(
            leading: const Icon(Icons.notifications_off_outlined, color: AppTheme.incoming),
            title: const Text('Больше не напоминать', style: TextStyle(color: AppTheme.text)),
            subtitle: const Text('Его просьбы в эту группу вы больше не увидите', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); run(() => _svc!.ignoreRequester(g.id, r), 'Готово, напоминаний не будет'); },
          ),
        ]),
      ),
    );
  }

  void _entryMenu(GroupInfo g, GroupEntry e) {
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          if (g.canModerate)
            ListTile(
              leading: const Icon(Icons.delete_outline, color: AppTheme.incoming),
              title: const Text('Стереть у всех', style: TextStyle(color: AppTheme.text)),
              subtitle: const Text('Запись удаляется с узла и у участников', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () async {
                Navigator.pop(ctx);
                try {
                  await _svc!.moderate(g.id, 'delete_entry', seq: e.seq);
                  await _load();
                } catch (err) { _toast('$err'); }
              },
            ),
          if (g.canModerate && e.from != g.myAlias)
            ListTile(
              leading: const Icon(Icons.volume_off, color: AppTheme.textSecondary),
              title: Text('Заглушить ${_svc!.displayName(g.id, e.from)}', style: const TextStyle(color: AppTheme.text)),
              onTap: () async {
                Navigator.pop(ctx);
                try { await _svc!.moderate(g.id, 'mute', alias: e.from); _toast('Заглушён'); } catch (err) { _toast('$err'); }
              },
            ),
        ]),
      ),
    );
  }
}

class _GroupBubble extends StatelessWidget {
  final GroupEntry entry;
  final bool mine;
  final String author;
  final VoidCallback? onLongPress;
  final VoidCallback? onOpen;
  final String? sizeLabel;
  final double? progress;
  const _GroupBubble({required this.entry, required this.mine, required this.author, this.onLongPress, this.onOpen, this.sizeLabel, this.progress});

  @override
  Widget build(BuildContext context) {
    final time = DateFormat('HH:mm').format(DateTime.fromMillisecondsSinceEpoch(entry.ts));
    final t = entry.body['t'], name = entry.body['name'];
    final text = entry.body['op'] == 'lost' && entry.kind == GroupKind.system
        ? '🔒 Сообщение недоступно: ключ этого поколения уже удалён с узла'
        : entry.body['op'] == 'bad' && entry.kind == GroupKind.system
            ? '⚠ Повреждённое сообщение: не прошло проверку'
            : (t is String ? t : (name is String ? name : ''));
    return Align(
      alignment: mine ? Alignment.centerRight : Alignment.centerLeft,
      child: GestureDetector(
        onLongPress: onLongPress,
        child: Container(
          margin: const EdgeInsets.symmetric(vertical: 3),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
          constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.78),
          decoration: BoxDecoration(
            color: AppTheme.bg,
            border: Border.all(color: mine ? AppTheme.accent.withOpacity(0.85) : AppTheme.incoming, width: 1.5),
            borderRadius: BorderRadius.circular(16),
          ),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            if (!mine) Text(author, style: const TextStyle(color: AppTheme.incoming, fontSize: 12, fontWeight: FontWeight.bold)),
            if (entry.body['f'] is String)
              InkWell(
                onTap: progress == null ? onOpen : null,
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  Icon(entry.kind == GroupKind.image ? Icons.image_outlined : entry.kind == GroupKind.audio ? Icons.music_note : entry.kind == GroupKind.video ? Icons.movie_outlined : Icons.insert_drive_file_outlined, color: AppTheme.accent),
                  const SizedBox(width: 8),
                  Flexible(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text('${entry.body['n'] ?? 'файл'}', style: const TextStyle(color: AppTheme.text, fontSize: 14), overflow: TextOverflow.ellipsis),
                    Text(progress != null ? 'загрузка ${(progress! * 100).round()}%' : (sizeLabel ?? ''), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                  ])),
                ]),
              )
            else
              Text(text, style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            Align(alignment: Alignment.centerRight, child: Text(time, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 10))),
          ]),
        ),
      ),
    );
  }
}

/// Просьба вступить как служебное сообщение в чате: видят только владелец и модераторы, обычным участникам её не показывают.
class _RequestCard extends StatelessWidget {
  final JoinRequest request;
  final VoidCallback onTap;
  const _RequestCard({required this.request, required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.centerLeft,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          margin: const EdgeInsets.symmetric(vertical: 3),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.78),
          decoration: BoxDecoration(
            color: AppTheme.incoming.withOpacity(0.10),
            border: Border.all(color: AppTheme.incoming.withOpacity(0.7), style: BorderStyle.solid, width: 1.2),
            borderRadius: BorderRadius.circular(16),
          ),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Row(mainAxisSize: MainAxisSize.min, children: [
              const Icon(Icons.how_to_reg_outlined, color: AppTheme.incoming, size: 18),
              const SizedBox(width: 6),
              Flexible(child: Text(request.name.isEmpty ? 'Новая просьба' : request.name, style: const TextStyle(color: AppTheme.incoming, fontSize: 13, fontWeight: FontWeight.bold))),
            ]),
            const SizedBox(height: 4),
            const Text('просит добавить его в группу или прислать приглашение', style: TextStyle(color: AppTheme.text, fontSize: 14)),
            const SizedBox(height: 4),
            const Text('служебное: видят владелец и модераторы · нажмите, чтобы ответить', style: TextStyle(color: AppTheme.textSecondary, fontSize: 10)),
          ]),
        ),
      ),
    );
  }
}
