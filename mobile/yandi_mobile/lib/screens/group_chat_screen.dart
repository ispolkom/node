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
import '../content_policy.dart';
import '../widgets/link_guard.dart';
import '../widgets/secure_keyboard.dart';
import '../widgets/message_menu.dart';
import 'package:flutter/services.dart';
import 'dart:convert';
import '../services/storage_service.dart';
import 'group_info_screen.dart';
import 'chat_files_screen.dart';
import '../models/file_offer.dart';
import '../services/file_transfer_service.dart';
import '../services/voice_recorder.dart';
import '../widgets/inline_media.dart';
import '../widgets/recipients_sheet.dart';
import 'package:path_provider/path_provider.dart' as pp;
import '../l10n.dart';

/// Звено цепочки ответов: запись (seq) либо, если её уже нет, то, что осталось в карточке (known == false).
typedef ThreadItem = ({int? seq, String author, String text, bool known});

/// Цепочка ответов от первого сообщения до записи [e]: у каждой карточки-ответа якорь `r.s` на то, на что отвечали.
/// Защищена от петель; если родитель не загружен или стёрт, цепочка начинается с выдержки из карточки.
List<ThreadItem> replyChain(List<GroupEntry> entries, GroupEntry e, String Function(String from) nameOf, String Function(GroupEntry) excerpt) {
  final bySeq = {for (final x in entries) x.seq: x};
  final out = <ThreadItem>[];
  final seen = <int>{};
  GroupEntry? cur = e;
  while (cur != null && seen.add(cur.seq)) {
    out.add((seq: cur.seq, author: nameOf(cur.from), text: excerpt(cur), known: true));
    final r = cur.body['r'];
    if (r is! Map) break;
    final s = r['s'];
    final parent = s is int ? bySeq[s] : null;
    if (parent == null) {
      if (r['t'] is String) out.add((seq: null, author: r['a'] is String ? r['a'] as String : '', text: r['t'] as String, known: false));
      break;
    }
    cur = parent;
  }
  return out.reversed.toList();
}

/// Разделы-фильтры группы: вид записи → подпись.
List<(int?, String)> get groupSections => <(int?, String)>[
  (null, tr('Все')),
  (GroupKind.image, tr('Картинки')),
  (GroupKind.file, tr('Файлы')),
  (GroupKind.audio, tr('Аудио')),
  (GroupKind.video, tr('Видео')),
  (GroupKind.link, tr('Ссылки')),
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
  final Set<int> _selected = {};                // выбранные записи (номера)
  final Map<int, Set<String>> _fwdTo = {};      // кому уже переслана запись в этом сеансе (предел 20)
  bool get _selecting => _selected.isNotEmpty;
  late final VoiceRecording _rec = VoiceRecording(
    voiceBackendFactory(),
    () async => '${(await pp.getTemporaryDirectory()).path}/voice_${DateTime.now().millisecondsSinceEpoch}.m4a',
  )..addListener(() { if (mounted) setState(() {}); });
  bool _sendingVoice = false;

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
    final app = context.read<AppState>();
    _rec.stop().then((r) { if (r.$1 != null) app.dropPickerCopy(r.$1!); }).whenComplete(_rec.dispose);
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
    // какие файлы уже лежат у меня: для них в сообщении показывается плеер/превью, а не «скачать»
    for (final e in list) {
      if (e.body['f'] is String && !_paths.containsKey(_cacheId(e.body))) {
        _localPath(e.body).then((p) { if (p != null && mounted) setState(() {}); });
      }
    }
    if (list.length != _shownCount) {
      _shownCount = list.length;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients) _scroll.jumpTo(_scroll.position.maxScrollExtent);
      });
    }
  }

  String _fmtSize(int b) => b >= 1 << 20 ? trf('{n} МБ', {'n': (b / (1 << 20)).toStringAsFixed(1)}) : trf('{n} КБ', {'n': (b / 1024).ceil()});

  Future<void> _attach() async {
    final r = await FilePicker.platform.pickFiles(allowMultiple: false);
    final path = r?.files.single.path;
    if (path == null) return;
    await _sendFilePath(path, r!.files.single.name);
  }

  /// Отправить файл в группу (выбранный или записанное голосовое); открытая копия затирается при любом исходе.
  Future<void> _sendFilePath(String path, String name) async {
    final svc = _svc!;
    // вся работа с открытой копией — внутри try/finally: и отказ по имени/содержимому/правилам группы, и успех стирают её
    try {
      final bad = await attachmentVerdict(path, name);
      if (bad != null) { _toast('«$name»: $bad'); return; }
      final kind = GroupFiles.kindForName(name);
      if (!svc.rules(widget.gid).allows(kind)) { _toast(tr('В этой группе запрещён такой вид файлов')); return; }
      setState(() => _progress['up'] = 0);
      final d = await GroupFiles(svc.api, vault: await VaultService.instance.files()).upload(widget.gid, File(path), name, onProgress: (done, total) {
        if (mounted) setState(() => _progress['up'] = done / total);
      });
      try {
        await svc.send(widget.gid, kind, d);
      } catch (e) {
        try { await svc.api.fileDelete(widget.gid, d['f'] as String); } catch (_) {}   // запись не ушла — файл на узле не нужен
        rethrow;
      }
      // свой файл остаётся у меня зашифрованным под именем, привязанным к группе и описанию файла; открытая копия из выбора файла затирается
      try {
        final vf = await context.read<AppState>().vaultFiles();
        _paths[_cacheId(d)] = (await vf.importAs(path, _cacheId(d), name: name)).path;
        await context.read<AppState>().dropPickerCopy(path);
      } catch (_) {}
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast(trf('Файл не отправлен: {e}', {'e': e}));
    } finally {
      if (mounted) setState(() => _progress.remove('up'));
      // открытая копия из выбора файла затирается при любом исходе (запрет вида файла, отказ, успех)
      try { await context.read<AppState>().dropPickerCopy(path); } catch (_) {}
    }
  }

  /// Имя копии в хранилище привязано к группе, номеру файла И его описанию (хэш, размер): участник, который выпустил новое сообщение с чужим
  /// описанием под уже знакомым номером, не получает прежний файл — копия под другим описанием не находится и скачивается с проверкой хэша.
  String _cacheId(Map<String, dynamic> d) => FileVault.safeId('grp:${widget.gid}:${d['f']}:${d['h']}:${d['z']}');

  Future<String?> _localPath(Map<String, dynamic> d) async {
    final key = _cacheId(d);
    final mem = _paths[key];
    if (mem != null && File(mem).existsSync()) return mem;
    final p = (await context.read<AppState>().vaultFiles()).pathFor(key);
    if (File(p).existsSync()) { _paths[key] = p; return p; }
    return null;
  }

  /// Скачивает файл группы в закрытую среду; путь к нему или null, если не вышло (причина — во всплывающей строке).
  Future<String?> _fetchFile(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    final key = _cacheId(d);
    final vf = await context.read<AppState>().vaultFiles();
    final out = File(vf.pathFor(key));
    final part = File('${out.path}.part');   // пока качается, файла в хранилище нет
    setState(() => _progress[fid] = 0);
    try {
      await GroupFiles(_svc!.api, vault: await VaultService.instance.files()).download(widget.gid, d, part, onProgress: (done, total) {
        if (mounted) setState(() => _progress[fid] = done / total);
      });
    } catch (e) {
      _toast(trf('Не скачано: {e}', {'e': e}));
      try { if (await part.exists()) await part.delete(); } catch (_) {}
      return null;
    } finally {
      if (mounted) setState(() => _progress.remove(fid));
    }
    await part.rename(out.path);
    _paths[key] = out.path;
    return out.path;
  }

  Future<void> _openFile(Map<String, dynamic> d) async {
    final fid = d['f'] as String;
    var path = await _localPath(d);
    if (path == null) {
      path = await _fetchFile(d);
      if (path == null) return;
    }
    final k = GroupFiles.kindForName('${d['n'] ?? ''}');
    if (k == GroupKind.audio || k == GroupKind.video) {
      if (mounted) setState(() {});   // у звука и видео своё воспроизведение прямо в сообщении
      return;
    }
    final v = await (await context.read<AppState>().vaultFiles()).describe(path);
    if (!mounted) return;
    await Navigator.push(context, MaterialPageRoute(builder: (_) => FileViewerScreen(file: v)));
  }

  Future<void> _send() async {
    var text = _ctrl.text.trim();
    if (text.isEmpty) return;
    final guarded = await guardLinks(context, text);
    if (!mounted) return;
    if (guarded == null) {
      if (findLink(text) != null) _ctrl.text = removeLinks(text);
      return;
    }
    text = guarded;
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
      // после отправки клавиатура (и обычная, и наша) убирается
      if (mounted) setState(_focus.unfocus);
      await svc.sync(only: widget.gid);
      await _load();
    } on GroupSendRefused catch (e) {
      _toast(e.reason);
    } catch (e) {
      _toast(trf('Не отправлено: {e}', {'e': e}));
    }
  }

  static String _excerpt(GroupEntry e) {
    final raw = '${e.body['t'] ?? e.body['n'] ?? ''}'.replaceAll(RegExp(r'\s+'), ' ').trim();
    return raw.runes.length <= 200 ? raw : String.fromCharCodes(raw.runes.take(200));
  }

  /// Роль автора записи по псевдониму (владелец, админ, модератор, участник; null — неизвестна).
  String? _authorRole(GroupInfo g, GroupEntry e) {
    if (e.from == g.myAlias) return g.role;
    return _svc?.roleOf(g.id, e.from);
  }

  bool _canDelete(GroupInfo g, GroupEntry e) => g.canDeleteEntryOf(_authorRole(g, e), mine: e.from == g.myAlias);

  /// Меню слов по нажатию или удержанию записи: сверху «Ответить», затем «Копировать», «Переслать», «Выбрать»; рядовой вместо
  /// удаления может «Скрыть сообщения этого пользователя»; удаляют владелец, админ и модератор — по матрице ролей.
  Future<void> _entryCommands(GroupInfo g, GroupEntry e, Offset at) async {
    final mine = e.from == g.myAlias;
    final hasText = e.body['t'] is String;
    final isFile = e.body['f'] is String;
    final role = _authorRole(g, e);
    final choice = await showMessageMenu(context, at, [
      cmdReply,
      if (e.body['r'] is Map) cmdThread,
      if (hasText) cmdCopy,
      if (hasText || isFile) cmdShare,
      cmdSelect,
      if (!mine) cmdHideAuthor,
      if (_canDelete(g, e)) cmdDelete,
      if (!mine && role != null && g.canActOnRole(role) && g.canModerate) cmdMute,
    ], reactions: mine || e.kind == GroupKind.system || !_svc!.rules(g.id).allows(GroupKind.reaction) ? const [] : [
      MenuReaction('1', '😊', coin: true, active: _svc!.myVote(g.id, e.seq) == 1),
      MenuReaction('-1', '😢', coin: true, active: _svc!.myVote(g.id, e.seq) == -1),
    ]);
    if (!mounted || choice == null) return;
    if (choice.startsWith('react:')) {
      try {
        await _svc!.vote(g.id, e.seq, int.parse(choice.substring(6)));
      } catch (err) {
        _toast('$err');
      }
      return;
    }
    switch (choice) {
      case cmdReply:
        setState(() => _replyTo = e);
        _focus.requestFocus();
      case cmdThread:
        await _showThread(g, e);
      case cmdCopy:
        await Clipboard.setData(ClipboardData(text: '${e.body['t']}'));
      case cmdShare:
        await _forwardEntries(g, [e]);
      case cmdSelect:
        setState(() => _selected.add(e.seq));
      case cmdHideAuthor:
        setState(() => _hidden.add(e.from));
        await _saveHidden();
      case cmdDelete:
        await _deleteEntries(g, [e]);
      case cmdMute:
        try { await _svc!.moderate(g.id, 'mute', alias: e.from); _toast(tr('Заглушён')); } catch (err) { _toast('$err'); }
    }
  }

  /// Стереть записи у всех участников группы (только по правам): с подтверждением.
  Future<void> _deleteEntries(GroupInfo g, List<GroupEntry> list) async {
    final allowed = list.where((e) => _canDelete(g, e)).toList();
    if (allowed.isEmpty) { _toast(tr('Эти сообщения вам удалять нельзя')); return; }
    final n = allowed.length;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(n == 1 ? tr('Удалить сообщение?') : trf('Удалить сообщения ({n})?', {'n': n}), style: const TextStyle(color: AppTheme.text)),
        content: Text(tr('Сообщение исчезнет у всех участников группы.'), style: TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'), style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Удалить'), style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    try {
      for (final e in allowed) { await _svc!.moderate(g.id, 'delete_entry', seq: e.seq); }
    } catch (err) {
      _toast('$err');
    }
    if (mounted) setState(_selected.clear);
    await _load();
  }

  /// Переслать записи (текст и файлы, что уже лежат у меня) до 5 контактам за раз; в группу-источник не пересылаем.
  Future<void> _forwardEntries(GroupInfo g, List<GroupEntry> list) async {
    if (list.isEmpty) return;
    final state = context.read<AppState>();
    final to = await pickRecipients(context,
        title: list.length == 1 ? tr('Переслать сообщение') : trf('Переслать сообщения ({n})', {'n': list.length}),
        contacts: forwardCandidates(state.contacts));
    if (to == null || to.isEmpty || !mounted) return;
    for (final e in list) {
      if ({...?_fwdTo[e.seq], ...to.map((c) => c.peerId)}.length > 20) {
        _toast(tr('Одно сообщение — не больше 20 получателей'));
        return;
      }
    }
    var skipped = 0;
    for (final e in [...list]..sort((a, b) => a.seq.compareTo(b.seq))) {
      final fid = e.body['f'];
      String? local;
      if (fid is String) {
        local = await _localPath(e.body);
        if (local == null) { skipped++; continue; }
      }
      _fwdTo.putIfAbsent(e.seq, () => {}).addAll(to.map((c) => c.peerId));
      for (final c in to) {
        if (local != null) {
          state.sendFile(c.peerId, local, name: '${e.body['n'] ?? tr('файл')}').then((err) { if (err != null && mounted) _toast('${e.body['n']} → ${c.displayName}: $err'); });
        } else {
          state.sendMessage(c.peerId, '${e.body['t'] ?? ''}');
        }
      }
    }
    if (mounted) {
      setState(_selected.clear);
      _toast(trf('Отправка → {names}', {'names': to.map((c) => c.displayName).join(', ')}) + (skipped > 0 ? trf('. Не скачанных файлов пропущено: {n}', {'n': skipped}) : ''));
    }
  }

  /// Записать и отправить голосовое сообщение: файлом m4a в группу, как обычное вложение.
  Future<void> _startVoice() async {
    final err = await _rec.start();
    if (err != null) _toast(err);
    _rec.onLimit = () => _finishVoice(send: true);
  }

  Future<void> _finishVoice({required bool send}) async {
    if (!_rec.active || _sendingVoice) return;
    _sendingVoice = true;
    final state = context.read<AppState>();
    final (path, seconds) = await _rec.stop();
    try {
      if (send && path != null && seconds >= 1) {
        final now = DateTime.now();
        String two(int n) => n.toString().padLeft(2, '0');
        await _sendFilePath(path, 'Голосовое_${now.year}${two(now.month)}${two(now.day)}_${two(now.hour)}${two(now.minute)}${two(now.second)}.m4a');
      } else if (send) {
        _toast(tr('Запись слишком короткая'));
      }
    } finally {
      if (path != null) await state.dropPickerCopy(path);
      _sendingVoice = false;
    }
  }

  int? _flash;   // запись, к которой только что перешли по карточке: подсвечивается на пару секунд

  void _flashSeq(int seq) {
    setState(() => _flash = seq);
    Future.delayed(const Duration(milliseconds: 1800), () { if (mounted && _flash == seq) setState(() => _flash = null); });
  }

  List<ThreadItem> _chainOf(GroupInfo g, GroupEntry e) =>
      replyChain(_entries, e, (from) => _svc!.displayName(g.id, from), _excerpt);

  Future<void> _showThread(GroupInfo g, GroupEntry e) async {
    final chain = _chainOf(g, e);
    final seq = await showModalBottomSheet<int>(
      context: context,
      backgroundColor: AppTheme.surface,
      isScrollControlled: true,
      builder: (ctx) => SafeArea(
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * 0.7),
          child: ListView(shrinkWrap: true, padding: const EdgeInsets.all(12), children: [
            Padding(
              padding: EdgeInsets.only(bottom: 8),
              child: Text(tr('Цепочка ответов: сверху — с чего всё началось'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ),
            for (var i = 0; i < chain.length; i++)
              Padding(
                padding: EdgeInsets.only(left: (i * 10.0).clamp(0, 60), bottom: 6),
                child: InkWell(
                  key: ValueKey('thread-$i'),
                  onTap: chain[i].seq == null ? null : () => Navigator.pop(ctx, chain[i].seq),
                  child: Container(
                    padding: const EdgeInsets.fromLTRB(10, 6, 10, 6),
                    decoration: BoxDecoration(
                      color: AppTheme.accent.withOpacity(i == chain.length - 1 ? 0.16 : 0.07),
                      border: const Border(left: BorderSide(color: AppTheme.accent, width: 3)),
                    ),
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text('${chain[i].author}${chain[i].known ? '' : tr(' · сообщение недоступно')}',
                          style: const TextStyle(color: AppTheme.accent, fontSize: 12, fontWeight: FontWeight.bold)),
                      Text(chain[i].text, maxLines: 4, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text)),
                    ]),
                  ),
                ),
              ),
          ]),
        ),
      ),
    );
    if (seq != null && mounted) _jumpToSeq(seq);
  }

  void _jumpToSeq(int seq) {
    _flashSeq(seq);
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
      return Scaffold(appBar: AppBar(title: Text(tr('Группа'))), body: Center(child: Text(tr('Группа недоступна'), style: TextStyle(color: AppTheme.textSecondary))));
    }
    final noKey = !state.groups!.hasProblem(g.id) && g.epoch == 0;
    final selectedEntries = _entries.where((e) => _selected.contains(e.seq)).toList();
    final canDeleteAll = selectedEntries.isNotEmpty && selectedEntries.every((e) => _canDelete(g, e));
    return Scaffold(
      appBar: _selecting
          ? AppBar(
              backgroundColor: AppTheme.surface,
              elevation: 0,
              leading: IconButton(icon: const Icon(Icons.close), onPressed: () => setState(_selected.clear)),
              title: Text(trf('Выбрано: {n}', {'n': _selected.length}), style: const TextStyle(fontSize: 16)),
              actions: [
                IconButton(
                  key: const ValueKey('group-forward-selected'),
                  tooltip: tr('Переслать'),
                  icon: const Icon(Icons.forward_to_inbox_outlined),
                  onPressed: () => _forwardEntries(g, selectedEntries),
                ),
                // рядовой участник стереть не может — только владелец, админ и модератор (по ролям авторов)
                if (canDeleteAll)
                  IconButton(
                    key: const ValueKey('group-delete-selected'),
                    tooltip: tr('Удалить'),
                    icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
                    onPressed: () => _deleteEntries(g, selectedEntries),
                  ),
              ],
            )
          : AppBar(
        title: InkWell(
          onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupInfoScreen(gid: g.id))),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(g.name, style: const TextStyle(fontSize: 16)),
            Text(trf('{n} уч. · вы: {alias}', {'n': g.members, 'alias': g.myAlias}), style: const TextStyle(fontSize: 11, color: AppTheme.textSecondary)),
          ]),
        ),
        actions: [IconButton(icon: const Icon(Icons.settings_outlined), tooltip: tr('Настройки группы'), onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupInfoScreen(gid: g.id))))],
      ),
      body: Column(children: [
        // навигационная строка: «Файлы группы» — папка только этой группы (то, что уже у меня; наружу не выносится)
        InkWell(
          key: const ValueKey('group-files-link'),
          onTap: () {
            final offers = <FileOffer>[
              for (final e in _entries)
                if (e.body['f'] is String)
                  FileOffer(
                    tid: e.body['f'] as String,
                    name: '${e.body['n'] ?? tr('файл')}',
                    size: e.body['z'] is int ? e.body['z'] as int : 0,
                    chunks: e.body['c'] is int ? e.body['c'] as int : 0,
                    mime: FileTransferService.guessMime('${e.body['n'] ?? ''}'),
                    key: '',
                  ),
            ];
            final bodies = {for (final e in _entries) if (e.body['f'] is String) e.body['f'] as String: e.body};
            Navigator.push(context, MaterialPageRoute(
              builder: (_) => ChatFilesScreen(title: g.name, shortId: g.id.substring(0, 8), offers: offers,
                  // не скачанные файлы группы тоже в папке: нажатие скачивает их в закрытую среду
                  onFetch: (o) async => bodies[o.tid] != null && await _fetchFile(bodies[o.tid]!) != null, emptyHint: tr('В этой папке пока нет файлов. Файлы группы, которые вы скачали или отправили, появятся здесь.'),
                  links: [
                    for (final e in _entries)
                      if (e.body['t'] is String) ...FileLink.inText(stripLinks(e.body['t'] as String), DateTime.fromMillisecondsSinceEpoch(e.ts)),
                  ]),
            ));
          },
          child: Container(
            width: double.infinity,
            color: AppTheme.surface,
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
            child: Row(children: [
              Icon(Icons.folder_outlined, size: 18, color: AppTheme.accent),
              SizedBox(width: 8),
              Text(tr('Файлы группы'), style: TextStyle(color: AppTheme.text, fontSize: 14)),
              Spacer(),
              Icon(Icons.chevron_right, size: 18, color: AppTheme.textSecondary),
            ]),
          ),
        ),
        if (svc.hasProblem(g.id))
          Container(
            width: double.infinity, color: AppTheme.incoming.withOpacity(0.15), padding: const EdgeInsets.all(8),
            child: Text(tr('Ключ владельца группы изменился. Не доверяйте новым ключам, пока не проверите владельца лично.'), style: TextStyle(color: AppTheme.incoming, fontSize: 12)),
          )
        else if (noKey)
          Container(
            width: double.infinity, color: AppTheme.surface, padding: const EdgeInsets.all(8),
            child: Text(tr('Ключ группы ещё не создан владельцем. Сообщения появятся, когда он откроет приложение.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
        Expanded(
          child: _entries.isEmpty && _requests.isEmpty
              ? Center(child: Text(_section == null ? tr('Сообщений пока нет') : tr('В этом разделе пусто'), style: const TextStyle(color: AppTheme.textSecondary)))
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
                      onTap: _selecting
                          ? () => setState(() => _selected.contains(e.seq) ? _selected.remove(e.seq) : _selected.add(e.seq))
                          : () => _entryCommands(g, e, tapAt),
                      // удержание — тот же список слов, что и по нажатию; при уже выбранных — переключает выбор
                      onLongPressStart: (d) => _selecting
                          ? setState(() => _selected.contains(e.seq) ? _selected.remove(e.seq) : _selected.add(e.seq))
                          : _entryCommands(g, e, d.globalPosition),
                      selected: _selected.contains(e.seq),
                      flash: _flash == e.seq,
                      localPath: e.body['f'] is String ? _paths[_cacheId(e.body)] : null,
                      entry: e,
                      mine: mine,
                      author: svc.displayName(g.id, e.from),
                      avatar: svc.memberAvatar(g.id, e.from),
                      coins: svc.hasVotes(g.id, e.seq) ? svc.coins(g.id, e.seq) : null,
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
              Expanded(child: Text(trf('Скрыто пользователей: {n}', {'n': _hidden.length}), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12))),
              GestureDetector(
                onTap: () async { setState(() => _hidden = {}); await _saveHidden(); },
                child: Text(tr('Показать скрытых'), style: TextStyle(color: AppTheme.accent, fontSize: 12, decoration: TextDecoration.underline)),
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
    String mmss(int s) => '${s ~/ 60}:${(s % 60).toString().padLeft(2, '0')}';
    if (_rec.active) {
      return Container(
        color: AppTheme.surface,
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
        child: SafeArea(
          top: false,
          child: Row(children: [
            IconButton(
              key: const ValueKey('voice-cancel'),
              tooltip: tr('Отменить запись'),
              icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
              onPressed: () => _finishVoice(send: false),
            ),
            const Icon(Icons.fiber_manual_record, color: Colors.redAccent, size: 14),
            const SizedBox(width: 8),
            Text(mmss(_rec.seconds), style: const TextStyle(color: AppTheme.text, fontSize: 16)),
            const SizedBox(width: 12),
            Expanded(child: Text(tr('Идёт запись…'), style: TextStyle(color: AppTheme.textSecondary))),
            GestureDetector(
              key: const ValueKey('voice-send'),
              onTap: () => _finishVoice(send: true),
              child: Container(
                width: 44, height: 44,
                decoration: const BoxDecoration(shape: BoxShape.circle, color: AppTheme.accent),
                child: const Icon(Icons.send, color: Colors.white, size: 20),
              ),
            ),
          ]),
        ),
      );
    }
    return Container(
      color: AppTheme.surface,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
      child: SafeArea(
        top: false,
        child: Row(children: [
          IconButton(
            tooltip: tr('Прикрепить файл'),
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
                hintText: denied
                    ? tr('Вы заглушены в этой группе')
                    : (!g.ownerPresent ? tr('Владелец группы отсутствует: писать может не получиться') : tr('Сообщение…')),
                hintStyle: const TextStyle(color: AppTheme.textSecondary),
                filled: true,
                fillColor: AppTheme.bg,
                border: OutlineInputBorder(borderRadius: BorderRadius.circular(24), borderSide: BorderSide.none),
                contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              ),
            ),
          ),
          // пока строка пуста — микрофон (голосовое сообщение), с текстом — «отправить»
          ValueListenableBuilder<TextEditingValue>(
            valueListenable: _ctrl,
            builder: (_, v, __) {
              final empty = v.text.trim().isEmpty;
              return IconButton(
                key: ValueKey(empty ? 'voice-mic' : 'send-text'),
                icon: Icon(empty ? Icons.mic : Icons.send, color: AppTheme.accent),
                onPressed: denied ? null : (empty ? _startVoice : _send),
              );
            },
          ),
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
          Padding(padding: const EdgeInsets.all(16), child: Text(trf('{n} просит приглашение в группу', {'n': r.name.isEmpty ? tr('Кто-то') : r.name}), style: const TextStyle(color: AppTheme.text, fontSize: 16))),
          ListTile(
            leading: const Icon(Icons.person_add_alt, color: AppTheme.accent),
            title: Text(tr('Добавить'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Ему уйдёт приглашение личным сообщением'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); run(() async {
              final code = await _svc!.newInvite(g.id, uses: 1, ttlHours: 72);
              await state.sendMessage(r.peerId, GroupInviteMsg.encode(g.id, code, g.name));
              await _svc!.closeRequest(g.id, r.id);
            }, tr('Приглашение отправлено')); },
          ),
          ListTile(
            leading: const Icon(Icons.close, color: AppTheme.textSecondary),
            title: Text(tr('Игнорировать'), style: TextStyle(color: AppTheme.text)),
            onTap: () { Navigator.pop(ctx); run(() => _svc!.closeRequest(g.id, r.id), tr('Просьба закрыта')); },
          ),
          ListTile(
            leading: const Icon(Icons.notifications_off_outlined, color: AppTheme.incoming),
            title: Text(tr('Больше не напоминать'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Его просьбы в эту группу вы больше не увидите'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () { Navigator.pop(ctx); run(() => _svc!.ignoreRequester(g.id, r), tr('Готово, напоминаний не будет')); },
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
  /// Картинка визитки автора (если он её раскрыл).
  final Uint8List? avatar;
  final VoidCallback? onTap;
  final void Function(TapDownDetails)? onTapDown;
  final VoidCallback? onOpen;
  final String? sizeLabel;
  final double? progress;
  final (String, String, int?)? quote;   // (автор, выдержка, номер записи)
  final VoidCallback? onQuoteTap;
  final void Function(LongPressStartDetails)? onLongPressStart;
  final bool selected;
  final bool flash;
  /// Баланс монеток одобрения под сообщением (null — голосов нет).
  final int? coins;
  /// Где файл лежит у меня (если уже скачан): для звука и видео — плеер прямо в сообщении.
  final String? localPath;
  const _GroupBubble({super.key, required this.entry, required this.mine, required this.author, this.avatar, this.onTap, this.onTapDown, this.onOpen, this.sizeLabel, this.progress, this.quote, this.onQuoteTap, this.onLongPressStart, this.selected = false, this.flash = false, this.localPath, this.coins});

  @override
  Widget build(BuildContext context) {
    final time = DateFormat('HH:mm').format(DateTime.fromMillisecondsSinceEpoch(entry.ts));
    final t = entry.body['t'], name = entry.body['name'];
    // файл с типом, который в сети не передаётся (чужой клиент): не показывается как файл и не скачивается
    final blockedFile = entry.body['f'] is String ? attachmentProblem('${entry.body['n'] ?? ''}') : null;
    final text = entry.body['op'] == 'lost' && entry.kind == GroupKind.system
        ? tr('🔒 Сообщение недоступно: ключ этого поколения уже удалён с узла')
        : entry.body['op'] == 'bad' && entry.kind == GroupKind.system
            ? tr('⚠ Повреждённое сообщение: не прошло проверку')
            : (t is String ? t : (name is String ? name : ''));
    return Align(
      alignment: mine ? Alignment.centerRight : Alignment.centerLeft,
      child: GestureDetector(
        onTap: onTap,
        onTapDown: onTapDown,
        onLongPressStart: onLongPressStart,
        child: Container(
          foregroundDecoration: selected ? BoxDecoration(color: AppTheme.accent.withOpacity(0.18), borderRadius: BorderRadius.circular(16)) : null,
          margin: const EdgeInsets.symmetric(vertical: 3),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
          constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.78),
          decoration: BoxDecoration(
            color: AppTheme.bg,
            border: Border.all(color: flash ? Colors.white : (mine ? AppTheme.accent.withOpacity(0.85) : AppTheme.incoming), width: flash ? 3 : 1.5),
            borderRadius: BorderRadius.circular(16),
          ),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            if (!mine)
              Row(mainAxisSize: MainAxisSize.min, children: [
                if (avatar != null) ...[
                  CircleAvatar(radius: 9, backgroundColor: AppTheme.surface, backgroundImage: MemoryImage(avatar!), onBackgroundImageError: (_, __) {}),
                  const SizedBox(width: 6),
                ],
                Flexible(child: Text(author, style: const TextStyle(color: AppTheme.incoming, fontSize: 12, fontWeight: FontWeight.bold))),
              ]),
            if (quote != null) QuoteBox(author: quote!.$1, text: quote!.$2, onTap: onQuoteTap),
            if (blockedFile != null)
              Text(trf('Файл не принят: {n}', {'n': blockedFile}), style: const TextStyle(color: AppTheme.incoming, fontSize: 13))
            else if (entry.body['f'] is String && localPath != null && progress == null && entry.kind == GroupKind.audio && entry.body['z'] is int && (entry.body['z'] as int) <= InlineAudio.maxInline)
              InlineAudio(
                key: ValueKey('ga-${entry.body['f']}'),
                path: localPath!,
                size: entry.body['z'] as int,
                caption: '${entry.body['n'] ?? ''}'.startsWith('Голосовое_') ? null : '${entry.body['n'] ?? ''}',
              )
            else if (entry.body['f'] is String && localPath != null && progress == null && entry.kind == GroupKind.video && entry.body['z'] is int)
              InlineVideo(
                key: ValueKey('gv-${entry.body['f']}'),
                path: localPath!,
                size: entry.body['z'] as int,
                mime: FileTransferService.guessMime('${entry.body['n'] ?? ''}'),
              )
            else if (entry.body['f'] is String)
              InkWell(
                onTap: progress == null ? onOpen : null,
                child: Row(mainAxisSize: MainAxisSize.min, children: [
                  Icon(entry.kind == GroupKind.image ? Icons.image_outlined : entry.kind == GroupKind.audio ? Icons.music_note : entry.kind == GroupKind.video ? Icons.movie_outlined : Icons.insert_drive_file_outlined, color: AppTheme.accent),
                  const SizedBox(width: 8),
                  Flexible(child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text('${entry.body['n'] ?? tr('файл')}', style: const TextStyle(color: AppTheme.text, fontSize: 14), overflow: TextOverflow.ellipsis),
                    Text(progress != null ? trf('загрузка {n}%', {'n': (progress! * 100).round()}) : (sizeLabel ?? ''), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                  ])),
                ]),
              )
            else
              Text(text, style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            Row(mainAxisSize: MainAxisSize.min, mainAxisAlignment: MainAxisAlignment.end, children: [
              if (coins != null) ...[
                _CoinChip(key: ValueKey('coins-${entry.seq}'), value: coins!),
                const SizedBox(width: 10),
              ],
              Text(time, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 10)),
            ]),
          ]),
        ),
      ),
    );
  }
}

/// Монетка и баланс под сообщением; при изменении числа над монеткой на миг всплывает «+1» (вверх) или «−1» (вниз).
class _CoinChip extends StatefulWidget {
  final int value;
  const _CoinChip({super.key, required this.value});

  @override
  State<_CoinChip> createState() => _CoinChipState();
}

class _CoinChipState extends State<_CoinChip> with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(vsync: this, duration: const Duration(milliseconds: 700));
  int _delta = 0;

  @override
  void didUpdateWidget(_CoinChip old) {
    super.didUpdateWidget(old);
    if (old.value != widget.value) {
      _delta = widget.value - old.value;
      _c.forward(from: 0);
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final v = widget.value;
    return AnimatedBuilder(
      animation: _c,
      builder: (_, __) {
        final t = _c.value;
        final up = _delta >= 0;
        return Stack(clipBehavior: Clip.none, alignment: Alignment.center, children: [
          Row(mainAxisSize: MainAxisSize.min, children: [
            Transform.scale(scale: _c.isAnimating ? 1 + 0.25 * (1 - (2 * t - 1).abs()) : 1, child: const CoinIcon(size: 15)),
            const SizedBox(width: 4),
            Text(v > 0 ? '+$v' : '$v', style: TextStyle(color: v < 0 ? AppTheme.incoming : AppTheme.text, fontSize: 12, fontWeight: FontWeight.bold)),
          ]),
          if (_c.isAnimating && _delta != 0)
            Positioned(
              top: -4 + (up ? -22 * t : 22 * t),
              child: Opacity(
                opacity: 1 - t,
                child: Text(up ? '+$_delta' : '−${-_delta}', style: TextStyle(color: up ? const Color(0xFFE0A100) : AppTheme.incoming, fontSize: 13, fontWeight: FontWeight.bold)),
              ),
            ),
        ]);
      },
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
              Flexible(child: Text(request.name.isEmpty ? tr('Новая просьба') : request.name, style: const TextStyle(color: AppTheme.incoming, fontSize: 13, fontWeight: FontWeight.bold))),
            ]),
            const SizedBox(height: 4),
            Text(tr('просит добавить его в группу или прислать приглашение'), style: TextStyle(color: AppTheme.text, fontSize: 14)),
            const SizedBox(height: 4),
            Text(tr('служебное: видят владелец и модераторы · нажмите, чтобы ответить'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 10)),
          ]),
        ),
      ),
    );
  }
}
