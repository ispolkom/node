import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../crypto/group_crypto.dart';
import '../models/group.dart';
import '../services/app_state.dart';
import '../services/group_service.dart';
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
    if (!mounted) return;
    setState(() => _entries = list);
    if (list.length != _shownCount) {
      _shownCount = list.length;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients) _scroll.jumpTo(_scroll.position.maxScrollExtent);
      });
    }
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
        actions: [IconButton(icon: const Icon(Icons.info_outline), tooltip: 'О группе', onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupInfoScreen(gid: g.id))))],
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
          child: _entries.isEmpty
              ? Center(child: Text(_section == null ? 'Сообщений пока нет' : 'В этом разделе пусто', style: const TextStyle(color: AppTheme.textSecondary)))
              : ListView.builder(
                  controller: _scroll,
                  padding: const EdgeInsets.all(12),
                  itemCount: _entries.length,
                  itemBuilder: (_, i) {
                    final e = _entries[i];
                    final mine = e.from == g.myAlias;
                    return _GroupBubble(
                      entry: e,
                      mine: mine,
                      author: svc.displayName(g.id, e.from),
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
  const _GroupBubble({required this.entry, required this.mine, required this.author, this.onLongPress});

  @override
  Widget build(BuildContext context) {
    final time = DateFormat('HH:mm').format(DateTime.fromMillisecondsSinceEpoch(entry.ts));
    final text = entry.body['t'] as String? ?? entry.body['name'] as String? ?? '';
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
            Text(text, style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            Align(alignment: Alignment.centerRight, child: Text(time, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 10))),
          ]),
        ),
      ),
    );
  }
}
