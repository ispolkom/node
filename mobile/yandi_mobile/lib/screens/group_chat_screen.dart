import 'dart:io';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../crypto/file_vault.dart';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../models/group_invite.dart';
import '../services/app_state.dart';
import '../services/group_files.dart';
import '../services/group_service.dart';
import '../services/vault_service.dart';
import 'file_viewer_screen.dart';
import '../theme.dart';
import '../widgets/secure_keyboard.dart';
import '../widgets/message_menu.dart';
import 'package:flutter/services.dart';
import 'dart:convert';
import '../services/storage_service.dart';
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
  GroupEntry? _replyTo;                         // запись, на которую отвечаем
  Set<String> _hidden = {};                     // псевдонимы, чьи сообщения скрыты у меня (только на этом устройстве)
  final Map<int, GlobalKey> _anchors = {};

  @override
  void initState() {
    super.initState();
    _focus.addListener(() => setState(() {}));
    _loadHidden();
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

  String get _hiddenKey => 'group_hidden:${widget.gid}';

  Future<void> _loadHidden() async {
    try {
      final raw = await StorageService.secureRead(_hiddenKey);
      if (raw != null && raw.isNotEmpty && mounted) setState(() => _hidden = (jsonDecode(raw) as List).map((e) => '$e').toSet());
    } catch (_) {}
  }

  Future<void> _saveHidden() async {
    try { await StorageService.secureWrite(_hiddenKey, jsonEncode(_hidden.toList())); } catch (_) {}
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
      final d = await GroupFiles(svc.api, vault: await VaultService.instance.files()).upload(widget.gid, File(path), name, onProgress: (done, total) {
        if (mounted) setState(() => _progress['up'] = done / total);
      });
      try {
        await svc.send(widget.gid, kind, d);
      } catch (e) {
        try { await svc.api.fileDelete(widget.gid, d['f'] as String); } catch (_) {}   // запись не ушла — файл на узле не нужен
        rethrow;
      }
      // свой файл остаётся у меня зашифрованным под номером файла; открытая копия из выбора файла затирается
      try {
        final vf = await context.read<AppState>().vaultFiles();
        _paths[d['f'] as String] = (await vf.importAs(path, d['f'] as String, name: name)).path;
        await context.read<AppState>().dropPickerCopy(path);
      } catch (_) {}
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast('Файл не отправлен: $e');
    } finally {
      if (mounted) setState(() => _progress.remove('up'));
      // открытая копия из выбора файла затирается при любом исходе (запрет вида файла, отказ, успех)
      try { await context.read<AppState>().dropPickerCopy(path); } catch (_) {}
    }
  }

  Future<String?> _localPath(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    final mem = _paths[fid];
    if (mem != null && File(mem).existsSync()) return mem;
    final p = (await context.read<AppState>().vaultFiles()).pathFor(FileVault.safeId(fid));
    if (File(p).existsSync()) { _paths[fid] = p; return p; }
    return null;
  }

  Future<void> _openFile(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    var path = await _localPath(d);
    if (path == null) {
      final vf = await context.read<AppState>().vaultFiles();
      final out = File(vf.pathFor(FileVault.safeId(fid)));
      final part = File('${out.path}.part');   // пока качается, файла в хранилище нет
      setState(() => _progress[fid] = 0);
      try {
        await GroupFiles(_svc!.api, vault: await VaultService.instance.files()).download(widget.gid, d, part, onProgress: (done, total) {
          if (mounted) setState(() => _progress[fid] = done / total);
        });
      } catch (e) {
        _toast('Не скачано: $e');
        return;
      } finally {
        if (mounted) setState(() => _progress.remove(fid));
      }
      await part.rename(out.path);
      _paths[fid] = out.path;
      path = out.path;
    }
    final v = await (await context.read<AppState>().vaultFiles()).describe(path);
    if (!mounted) return;
    await Navigator.push(context, MaterialPageRoute(builder: (_) => FileViewerScreen(file: v)));
  }

  Future<void> _send() async {
    final text = _ctrl.text.trim();
    if (text.isEmpty) return;
    final svc = _svc!;
    try {
      final q = _replyTo;
      await svc.send(widget.gid, kindOfText(text), {
        't': text,
        // ответ-цитата: s — номер записи, a — автор, t — выдержка (как у узла и оболочки)
        if (q != null) 'r': {'s': q.seq, 'a': svc.displayName(widget.gid, q.from), 't': _excerpt(q)},
      });
      _ctrl.clear();
      _replyTo = null;
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast('Не отправлено: $e');
    }
  }

  static String _excerpt(GroupEntry e) {
    final raw = '${e.body['t'] ?? e.body['n'] ?? ''}'.replaceAll(RegExp(r'\s+'), ' ').trim();
    return raw.runes.length <= 200 ? raw : String.fromCharCodes(raw.runes.take(200));
  }

  /// Меню слов по нажатию на запись: Ответить, Копировать, Скрыть сообщения этого пользователя, а по правам — Стереть у всех, Заглушить.
  Future<void> _entryCommands(GroupInfo g, GroupEntry e, Offset at) async {
    final mine = e.from == g.myAlias;
    final hasText = e.body['t'] is String;
    final choice = await showMessageMenu(context, at, [
      cmdReply,
      if (hasText) cmdCopy,
      if (!mine) cmdHideAuthor,
      if (g.canModerate) cmdEraseAll,
      if (g.canModerate && !mine) cmdMute,
    ]);
    if (!mounted || choice == null) return;
    switch (choice) {
      case cmdReply:
        setState(() => _replyTo = e);
        _focus.requestFocus();
      case cmdCopy:
        await Clipboard.setData(ClipboardData(text: '${e.body['t']}'));
      case cmdHideAuthor:
        setState(() => _hidden.add(e.from));
        await _saveHidden();
      case cmdEraseAll:
        try { await _svc!.moderate(g.id, 'delete_entry', seq: e.seq); await _load(); } catch (err) { _toast('$err'); }
      case cmdMute:
        try { await _svc!.moderate(g.id, 'mute', alias: e.from); _toast('Заглушён'); } catch (err) { _toast('$err'); }
    }
  }

  void _jumpToSeq(int seq) {
    final ctx = _anchors[seq]?.currentContext;
    if (ctx != null) { Scrollable.ensureVisible(ctx, duration: const Duration(milliseconds: 250), alignment: 0.3); return; }
    final i = _entries.indexWhere((x) => x.seq == seq);
    if (i < 0 || !_scroll.hasClients) return;
    final frac = _entries.length <= 1 ? 0.0 : i / (_entries.length - 1);
    _scroll.jumpTo((_scroll.position.maxScrollExtent * frac).clamp(0.0, _scroll.position.maxScrollExtent));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final c = _anchors[seq]?.currentContext;
      if (c != null) Scrollable.ensureVisible(c, duration: const Duration(milliseconds: 150), alignment: 0.3);
    });
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
                    if (!mine && _hidden.contains(e.from)) return const SizedBox.shrink();
                    final r = e.body['r'];
                    var tapAt = Offset.zero;
                    return _GroupBubble(
                      key: _anchors.putIfAbsent(e.seq, () => GlobalKey()),
                      quote: r is Map && r['t'] is String ? (r['a'] is String ? r['a'] as String : '', r['t'] as String, r['s'] is int ? r['s'] as int : null) : null,
                      onQuoteTap: r is Map && r['s'] is int ? () => _jumpToSeq(r['s'] as int) : null,
                      onTapDown: (d) => tapAt = d.globalPosition,
                      onTap: () => _entryCommands(g, e, tapAt),
                      entry: e,
                      mine: mine,
                      author: svc.displayName(g.id, e.from),
                      sizeLabel: e.body['z'] is int ? _fmtSize(e.body['z'] as int) : null,
                      progress: e.body['f'] is String ? _progress[e.body['f']] : null,
                      onOpen: e.body['f'] is String ? () => _openFile(e.body) : null,
                    );
                  },
                ),
        ),
        if (_hidden.isNotEmpty)
          Container(
            width: double.infinity, color: AppTheme.surface, padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            child: Row(children: [
              Expanded(child: Text('Скрыто пользователей: ${_hidden.length}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
              GestureDetector(
                onTap: () async { setState(() => _hidden = {}); await _saveHidden(); },
                child: const Text('Показать скрытых', style: TextStyle(color: AppTheme.accent, fontSize: 12, decoration: TextDecoration.underline)),
              ),
            ]),
          ),
        if (_replyTo != null)
          Container(
            color: AppTheme.surface,
            padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
            child: QuoteBox(author: svc.displayName(g.id, _replyTo!.from), text: _excerpt(_replyTo!), onClose: () => setState(() => _replyTo = null)),
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
}

class _GroupBubble extends StatelessWidget {
  final GroupEntry entry;
  final bool mine;
  final String author;
  final VoidCallback? onTap;
  final void Function(TapDownDetails)? onTapDown;
  final VoidCallback? onOpen;
  final String? sizeLabel;
  final double? progress;
  final (String, String, int?)? quote;   // (автор, выдержка, номер записи)
  final VoidCallback? onQuoteTap;
  const _GroupBubble({super.key, required this.entry, required this.mine, required this.author, this.onTap, this.onTapDown, this.onOpen, this.sizeLabel, this.progress, this.quote, this.onQuoteTap});

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
        onTap: onTap,
        onTapDown: onTapDown,
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
            if (quote != null) QuoteBox(author: quote!.$1, text: quote!.$2, onTap: onQuoteTap),
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
