import 'dart:async';
import 'dart:io';
import 'package:path_provider/path_provider.dart' as pp;
import 'package:crypto/crypto.dart' as crypto;
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'file_viewer_screen.dart';
import '../widgets/avatar_circle.dart';
import '../models/group_invite.dart';
import '../widgets/secure_keyboard.dart';
import 'groups_tab.dart' show joinInvited;
import 'group_chat_screen.dart';
import 'chat_files_screen.dart';
import '../services/voice_recorder.dart';
import '../widgets/inline_media.dart';
import '../widgets/recipients_sheet.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../models/file_offer.dart';
import '../models/message.dart';
import '../services/app_state.dart';
import '../services/file_transfer_service.dart' show maxFileSize;
import '../widgets/vault_image.dart';
import '../widgets/message_menu.dart';
import '../models/receipt.dart';
import 'package:flutter/services.dart';
import '../theme.dart';
import '../content_policy.dart';
import '../widgets/link_guard.dart';
import 'home_screen.dart' show showSaveContactDialog;

class ChatScreen extends StatefulWidget {
  final String peerId;
  final String title;
  const ChatScreen({super.key, required this.peerId, required this.title});

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final _inputCtrl = TextEditingController();
  final _scrollCtrl = ScrollController();
  final _inputFocus = FocusNode();
  final Set<String> _selected = {};
  String? _editingId;   // id редактируемого сообщения (null — обычная отправка)
  ChatMessage? _replyTo;   // сообщение, на которое отвечаем (цитата над ответом)
  final Map<String, GlobalKey> _anchors = {};

  bool get _selecting => _selected.isNotEmpty;

  Future<void> _showFingerprints() async {
    final state = context.read<AppState>();
    final bundle = await state.apiService.getPeerBundle(widget.peerId);
    if (!mounted) return;
    if (bundle == null || state.identity == null) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(state.nodeError ?? 'Проверенная связка ключей недоступна')));
      return;
    }
    await showDialog<void>(context:context,builder:(ctx)=>AlertDialog(
      title: const Text('Проверка ключей'),
      content: SingleChildScrollView(child: SelectableText(
        'Сравните отпечатки лично или по независимому каналу. Первое получение (TOFU) не подтверждает личность. При смене ключа отправка блокируется.\n\n'
        'Мой Ed25519 SHA-256:\n${crypto.sha256.convert(state.identity!.ed25519PublicBytes)}\n\n'
        'Собеседник Ed25519 SHA-256:\n${crypto.sha256.convert(bundle.ed25519)}')),
      actions:[TextButton(onPressed:()=>Navigator.pop(ctx),child:const Text('Закрыть'))],
    ));
  }

  void _startEdit(ChatMessage m) {
    setState(() {
      _editingId = m.id;
      _inputCtrl.text = m.text;
      _selected.clear();
    });
  }

  /// Кому уже переслано сообщение в этом сеансе (id → получатели): предел 20 на сообщение.
  final Map<String, Set<String>> _fwdTo = {};
  static const _maxPerMessage = 20;

  /// Переслать выбранные сообщения (текст и файлы) до 5 контактам за раз; в чат-источник не пересылаем.
  Future<void> _forwardMessages(List<ChatMessage> msgs) async {
    if (msgs.isEmpty) return;
    final state = context.read<AppState>();
    final contacts = forwardCandidates(state.contacts, exclude: widget.peerId);
    final to = await pickRecipients(context,
        title: msgs.length == 1 ? 'Переслать сообщение' : 'Переслать сообщения (${msgs.length})', contacts: contacts);
    if (to == null || to.isEmpty || !mounted) return;
    for (final m in msgs) {
      final all = {...?_fwdTo[m.id], ...to.map((c) => c.peerId)};
      if (all.length > _maxPerMessage) {
        _snack('Одно сообщение — не больше $_maxPerMessage получателей (уже ${_fwdTo[m.id]?.length ?? 0})');
        return;
      }
    }
    final ordered = [...msgs]..sort((a, b) => a.timestamp.compareTo(b.timestamp));
    var skipped = 0;
    for (final m in ordered) {
      final offer = FileOffer.tryParse(m.text);
      final local = offer == null ? null : state.localPathFor(offer);
      if (offer != null && local == null) { skipped++; continue; }
      _fwdTo.putIfAbsent(m.id, () => {}).addAll(to.map((c) => c.peerId));
      for (final c in to) {
        if (offer != null) {
          state.sendFile(c.peerId, local!, name: offer.name).then((err) { if (err != null && mounted) _snack('${offer.name} → ${c.displayName}: $err'); });
        } else {
          state.sendMessage(c.peerId, m.text);
        }
      }
    }
    if (mounted) {
      setState(() => _selected.clear());
      _snack(skipped > 0
          ? 'Отправка → ${to.map((c) => c.displayName).join(', ')}. Файлов без копии на телефоне пропущено: $skipped'
          : 'Отправка → ${to.map((c) => c.displayName).join(', ')}');
    }
  }

  void _cancelEdit() {
    setState(() { _editingId = null; _inputCtrl.clear(); });
  }

  /// Меню слов по нажатию на сообщение: Изменить, Ответить, Копировать, Удалить.
  Future<void> _messageMenu(ChatMessage m, Offset at) async {
    final plain = FileOffer.tryParse(m.text) == null && GroupInviteMsg.tryParse(m.text) == null;
    final isFile = FileOffer.tryParse(m.text) != null;
    final choice = await showMessageMenu(context, at, [
      cmdReply,                              // «Ответить» — самое верхнее
      if (m.outgoing && plain) cmdEdit,
      if (plain) cmdCopy,
      if (plain || isFile) cmdShare,         // «Переслать»
      cmdSelect,
      cmdDelete,
    ], reactions: [
      for (final e in MsgChannel.reactionSet)
        MenuReaction(e, e, active: context.read<AppState>().reactionsOf(m.id).$1 == e),
    ]);
    if (!mounted || choice == null) return;
    if (choice.startsWith('react:')) {
      final why = await context.read<AppState>().react(m, choice.substring(6));
      if (why != null) _snack(why);
      return;
    }
    switch (choice) {
      case cmdEdit:
        _startEdit(m);
      case cmdReply:
        setState(() => _replyTo = m);
        _inputFocus.requestFocus();
      case cmdCopy:
        await Clipboard.setData(ClipboardData(text: m.text));
      case cmdShare:
        await _forwardMessages([m]);
      case cmdSelect:
        setState(() => _selected.add(m.id));
      case cmdDelete:
        setState(() => _selected..clear()..add(m.id));
        await _deleteSelected();
    }
  }

  /// Якорь: прокрутить к сообщению, на которое ответили (если оно уже построено; иначе — примерно, затем точно).
  void _jumpTo(ChatMessage target, List<ChatMessage> all) {
    final ctx = _anchors[target.id]?.currentContext;
    if (ctx != null) {
      Scrollable.ensureVisible(ctx, duration: const Duration(milliseconds: 250), alignment: 0.3);
      return;
    }
    final i = all.indexWhere((x) => x.id == target.id);
    if (i < 0 || !_scrollCtrl.hasClients) return;
    final frac = all.length <= 1 ? 0.0 : i / (all.length - 1);
    _scrollCtrl.jumpTo((_scrollCtrl.position.maxScrollExtent * frac).clamp(0.0, _scrollCtrl.position.maxScrollExtent));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final c = _anchors[target.id]?.currentContext;
      if (c != null) Scrollable.ensureVisible(c, duration: const Duration(milliseconds: 150), alignment: 0.3);
    });
  }

  void _toggleSelect(String id) {
    setState(() {
      if (!_selected.remove(id)) _selected.add(id);
    });
  }

  Future<void> _deleteSelected() async {
    final n = _selected.length;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text('Удалить $n ${_plural(n)}?', style: const TextStyle(color: AppTheme.text)),
        content: const Text('Удалится только у вас. У собеседника останется.',
            style: TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(onPressed: () => Navigator.pop(ctx, true),
              child: const Text('Удалить', style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    if (ok != true) return;
    await context.read<AppState>().deleteMessages(widget.peerId, Set.of(_selected));
    if (mounted) setState(() => _selected.clear());
  }

  bool _singleOutgoing(BuildContext context) {
    if (_selected.length != 1) return false;
    final id = _selected.first;
    final msgs = context.read<AppState>().messagesFor(widget.peerId);
    final m = msgs.where((x) => x.id == id);
    return m.isNotEmpty && m.first.outgoing && FileOffer.tryParse(m.first.text) == null;
  }

  static String _plural(int n) {
    final a = n % 100, b = n % 10;
    if (a >= 11 && a <= 14) return 'сообщений';
    if (b == 1) return 'сообщение';
    if (b >= 2 && b <= 4) return 'сообщения';
    return 'сообщений';
  }

  @override
  void initState() {
    super.initState();
    _inputFocus.addListener(() { if (mounted) setState(() {}); });   // панель своей клавиатуры показывается, пока в поле фокус
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final state = context.read<AppState>();
      state.activeChatPeerId = widget.peerId;
      state.loadChatHistory(widget.peerId).then((_) { if (mounted) state.markChatRead(widget.peerId); });
    });
  }

  @override
  void dispose() {
    final state = context.read<AppState>();
    state.activeChatPeerId = null;
    // запись оборвана закрытием экрана: останавливаем, выбрасываем и затираем открытую копию
    _rec.stop().then((r) { if (r.$1 != null) state.dropPickerCopy(r.$1!); }).whenComplete(_rec.dispose);
    _inputCtrl.dispose();
    _inputFocus.dispose();
    _scrollCtrl.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    var text = _inputCtrl.text.trim();
    if (text.isEmpty) return;
    // ссылки в YANDI не передаются: предлагаем удалить (текст остаётся в поле, пока человек не решил)
    final guarded = await guardLinks(context, text);
    if (!mounted) return;
    if (guarded == null) {
      if (findLink(text) != null) _inputCtrl.text = removeLinks(text);
      return;
    }
    text = guarded;
    final editing = _editingId;
    _inputCtrl.clear();
    // после отправки клавиатура (и обычная, и наша) убирается
    setState(_inputFocus.unfocus);
    if (editing != null) {
      context.read<AppState>().editMessage(widget.peerId, editing, text);
      setState(() => _editingId = null);
      return;
    }
    final quote = _replyTo;
    setState(() => _replyTo = null);
    context.read<AppState>().sendMessage(widget.peerId, text, replyTo: quote);
    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());
  }

  void _snack(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _call({required bool video}) async {
    final err = await context.read<AppState>().startCall(widget.peerId, video: video);
    if (err != null) _snack(err);
  }

  /// Скрепка: выбрать файл и отправить его (зашифрованно, только на своё устройство).
  // ── Голосовое сообщение ────────────────────────────────────────────────────
  late final VoiceRecording _rec = VoiceRecording(
    voiceBackendFactory(),
    () async => '${(await pp.getTemporaryDirectory()).path}/voice_${DateTime.now().millisecondsSinceEpoch}.m4a',
  )..addListener(() { if (mounted) setState(() {}); });
  bool _sendingVoice = false;

  Future<void> _startVoice() async {
    final state = context.read<AppState>();
    if (!await state.canSendFiles(widget.peerId)) {
      _snack('Голосовые можно отправлять только на свои устройства (у этого собеседника нет ключа шифрования)');
      return;
    }
    _rec.onLimit = () => _finishVoice(send: true);
    final err = await _rec.start();
    if (err != null) _snack(err);
  }

  /// Остановить запись: отправить (send) или выбросить. Открытая копия записи в кеше затирается в любом случае.
  Future<void> _finishVoice({required bool send}) async {
    if (!_rec.active || _sendingVoice) return;
    _sendingVoice = true;
    final state = context.read<AppState>();
    final (path, seconds) = await _rec.stop();
    try {
      if (send && path != null && seconds >= 1) {
        final now = DateTime.now();
        String two(int n) => n.toString().padLeft(2, '0');
        final name = 'Голосовое_${now.year}${two(now.month)}${two(now.day)}_${two(now.hour)}${two(now.minute)}${two(now.second)}.m4a';
        final err = await state.sendFile(widget.peerId, path, name: name);
        if (err != null) _snack(err);
      } else if (send) {
        _snack('Запись слишком короткая');
      }
    } finally {
      if (path != null) await state.dropPickerCopy(path);
      _sendingVoice = false;
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());
  }

  Future<void> _pickAndSend() async {
    final state = context.read<AppState>();
    if (!await state.canSendFiles(widget.peerId)) {
      _snack('Файлы можно отправлять только на свои устройства (у этого собеседника нет ключа шифрования)');
      return;
    }
    final res = await FilePicker.platform.pickFiles(allowMultiple: true);
    if (res == null) return;
    try {
    for (final f in res.files) {
      final path = f.path;
      if (path == null) continue;
      if (f.size > maxFileSize) {
        _snack('«${f.name}» больше 1 ГБ: слишком большой');
        continue;
      }
      final bad = await attachmentVerdict(path, f.name);
      if (bad != null) {
        _snack('«${f.name}»: $bad');
        continue;
      }
      final err = await state.sendFile(widget.peerId, path, name: f.name);
      if (err != null) _snack(err);
    }
    } finally {
      // В том числе файл, отвергнутый по размеру до вызова sendFile.
      for (final f in res.files) { if (f.path != null) await state.dropPickerCopy(f.path!); }
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());
  }

  void _scrollToBottom() {
    if (_scrollCtrl.hasClients) {
      _scrollCtrl.animateTo(
        _scrollCtrl.position.maxScrollExtent,
        duration: const Duration(milliseconds: 200),
        curve: Curves.easeOut,
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final state    = context.watch<AppState>();
    final messages = state.messagesFor(widget.peerId);
    final contact  = state.contactFor(widget.peerId);
    final saved    = contact?.saved ?? false;
    final title    = (contact?.displayName.isNotEmpty ?? false) ? contact!.displayName : widget.title;

    WidgetsBinding.instance.addPostFrameCallback((_) {
      _scrollToBottom();
      state.markChatRead(widget.peerId);
    });

    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: _selecting
          ? AppBar(
              backgroundColor: AppTheme.surface,
              elevation: 0,
              iconTheme: const IconThemeData(color: AppTheme.text),
              leading: IconButton(
                icon: const Icon(Icons.close),
                onPressed: () => setState(() => _selected.clear()),
              ),
              title: Text('Выбрано: ${_selected.length}', style: const TextStyle(color: AppTheme.text, fontSize: 16)),
              actions: [
                if (_selected.length == 1 && _singleOutgoing(context))
                  IconButton(
                    tooltip: 'Изменить',
                    icon: const Icon(Icons.edit_outlined, color: AppTheme.text),
                    onPressed: () {
                      final id = _selected.first;
                      final m = context.read<AppState>().messagesFor(widget.peerId).firstWhere((x) => x.id == id);
                      _startEdit(m);
                    },
                  ),
                IconButton(
                  key: const ValueKey('chat-forward-selected'),
                  tooltip: 'Переслать',
                  icon: const Icon(Icons.forward_to_inbox_outlined, color: AppTheme.text),
                  onPressed: () {
                    final all = context.read<AppState>().messagesFor(widget.peerId);
                    _forwardMessages(all.where((x) => _selected.contains(x.id)).toList());
                  },
                ),
                IconButton(
                  tooltip: 'Удалить',
                  icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
                  onPressed: _deleteSelected,
                ),
              ],
            )
          : AppBar(
              backgroundColor: AppTheme.surface,
              elevation: 0,
              title: Row(children: [
                AvatarCircle(peerId: widget.peerId, label: title, radius: 18),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(title, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text, fontSize: 16)),
                      Text(widget.peerId.substring(0, 16) + '...',
                          style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                    ],
                  ),
                ),
              ]),
              iconTheme: const IconThemeData(color: AppTheme.text),
              actions: [
                IconButton(tooltip:'Проверить ключи',icon:const Icon(Icons.security),onPressed:_showFingerprints),
                IconButton(
                  tooltip: 'Позвонить',
                  icon: const Icon(Icons.call, color: AppTheme.accent),
                  onPressed: () => _call(video: false),
                ),
                IconButton(
                  tooltip: 'Видеозвонок',
                  icon: const Icon(Icons.videocam, color: AppTheme.accent),
                  onPressed: () => _call(video: true),
                ),
                if (!saved)
                  IconButton(
                    tooltip: 'Добавить в контакты',
                    icon: const Icon(Icons.person_add_alt_1, color: AppTheme.accent),
                    onPressed: () => showSaveContactDialog(context, widget.peerId, title),
                  ),
              ],
            ),
      body: Column(
        children: [
          // навигационная строка под шапкой: «Файлы чата» открывает папку файлов этого собеседника
          InkWell(
            key: const ValueKey('chat-files-link'),
            onTap: () {
              final offers = <FileOffer>[
                for (final m in state.messagesFor(widget.peerId))
                  if (FileOffer.tryParse(m.text) case final o?) o,
              ];
              Navigator.push(context, MaterialPageRoute(
                builder: (_) => ChatFilesScreen(
                  title: title,
                  shortId: widget.peerId.substring(0, 8),
                  offers: offers,
                  links: [
                    for (final m in state.messagesFor(widget.peerId))
                      if (FileOffer.tryParse(m.text) == null) ...FileLink.inText(stripLinks(m.text), m.timestamp),
                  ],
                ),
              ));
            },
            child: Container(
              width: double.infinity,
              color: AppTheme.surface,
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
              child: Row(children: const [
                Icon(Icons.folder_outlined, size: 18, color: AppTheme.accent),
                SizedBox(width: 8),
                Text('Файлы чата', style: TextStyle(color: AppTheme.text, fontSize: 14)),
                Spacer(),
                Icon(Icons.chevron_right, size: 18, color: AppTheme.textSecondary),
              ]),
            ),
          ),
          if (!saved)
            Material(
              color: AppTheme.accent.withOpacity(0.12),
              child: InkWell(
                onTap: () => showSaveContactDialog(context, widget.peerId, title),
                child: const Padding(
                  padding: EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                  child: Row(
                    children: [
                      Icon(Icons.info_outline, size: 18, color: AppTheme.accent),
                      SizedBox(width: 10),
                      Expanded(
                        child: Text('Этого человека нет в ваших контактах. Нажмите, чтобы добавить.',
                            style: TextStyle(color: AppTheme.text, fontSize: 13)),
                      ),
                      Icon(Icons.person_add_alt_1, size: 18, color: AppTheme.accent),
                    ],
                  ),
                ),
              ),
            ),
          Expanded(
            child: messages.isEmpty
                ? const Center(child: Text('Нет сообщений',
                    style: TextStyle(color: AppTheme.textSecondary)))
                : ListView.builder(
                    controller: _scrollCtrl,
                    padding: const EdgeInsets.all(12),
                    itemCount: messages.length,
                    itemBuilder: (_, i) {
                      final m = messages[i];
                      final target = m.replyText == null ? null : state.findReplyTarget(widget.peerId, m);
                      var tapAt = Offset.zero;
                      return GestureDetector(
                        key: _anchors.putIfAbsent(m.id, () => GlobalKey()),
                        // удержание — тот же список слов, что и по нажатию; при уже выбранных — переключает выбор
                        onLongPressStart: (d) => _selecting ? _toggleSelect(m.id) : _messageMenu(m, d.globalPosition),
                        onTapDown: (d) => tapAt = d.globalPosition,
                        onTap: _selecting ? () => _toggleSelect(m.id) : () => _messageMenu(m, tapAt),
                        child: Container(
                          color: _selected.contains(m.id) ? AppTheme.accent.withOpacity(0.18) : null,
                          child: Column(
                            crossAxisAlignment: m.outgoing ? CrossAxisAlignment.end : CrossAxisAlignment.start,
                            children: [
                              _MessageBubble(
                                msg: m,
                                edited: state.isEdited(m.id),
                                quoteAuthor: target == null ? null : (target.outgoing ? 'Вы' : title),
                                onQuoteTap: target == null ? null : () => _jumpTo(target, messages),
                              ),
                              Builder(builder: (_) {
                                final (mine, theirs) = state.reactionsOf(m.id);
                                if (mine.isEmpty && theirs.isEmpty) return const SizedBox.shrink();
                                return Padding(
                                  key: ValueKey('rx-${m.id}'),
                                  padding: const EdgeInsets.only(left: 10, right: 10, bottom: 2),
                                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                                    if (mine.isNotEmpty) Text(mine, style: const TextStyle(fontSize: 16)),
                                    if (mine.isNotEmpty && theirs.isNotEmpty && mine != theirs) const SizedBox(width: 4),
                                    if (theirs.isNotEmpty && theirs != mine) Text(theirs, style: const TextStyle(fontSize: 16)),
                                    if (mine.isNotEmpty && mine == theirs) const Text('×2', style: TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                                  ]),
                                );
                              }),
                            ],
                          ),
                        ),
                      );
                    },
                  ),
          ),
          if (_editingId != null)
            Container(
              color: AppTheme.surface,
              padding: const EdgeInsets.fromLTRB(16, 8, 8, 0),
              child: Row(children: [
                const Icon(Icons.edit_outlined, size: 16, color: AppTheme.accent),
                const SizedBox(width: 8),
                const Expanded(child: Text('Редактирование сообщения', style: TextStyle(color: AppTheme.accent, fontSize: 13))),
                IconButton(icon: const Icon(Icons.close, size: 18, color: AppTheme.textSecondary), onPressed: _cancelEdit),
              ]),
            ),
          if (_replyTo != null)
            Container(
              color: AppTheme.surface,
              padding: const EdgeInsets.fromLTRB(12, 8, 12, 0),
              child: QuoteBox(
                author: _replyTo!.outgoing ? 'Вы' : title,
                text: FileOffer.tryParse(_replyTo!.text)?.name ?? _replyTo!.text,
                onClose: () => setState(() => _replyTo = null),
              ),
            ),
          _InputBar(
            controller: _inputCtrl,
            focusNode: _inputFocus,
            ownKeyboard: context.watch<AppState>().secureKeyboard,
            onSend: _send,
            onAttach: _pickAndSend,
            recording: _rec.active,
            recordSeconds: _rec.seconds,
            onMic: _startVoice,
            onRecordSend: () => _finishVoice(send: true),
            onRecordCancel: () => _finishVoice(send: false),
          ),
          if (context.watch<AppState>().secureKeyboard && _inputFocus.hasFocus)
            SecureKeyboard(controller: _inputCtrl),
        ],
      ),
    );
  }
}

class _MessageBubble extends StatelessWidget {
  final ChatMessage msg;
  final bool edited;
  final String? quoteAuthor;
  final VoidCallback? onQuoteTap;
  const _MessageBubble({required this.msg, this.edited = false, this.quoteAuthor, this.onQuoteTap});

  @override
  Widget build(BuildContext context) {
    final isOut = msg.outgoing;
    final time  = DateFormat('HH:mm').format(msg.timestamp);
    final offer = FileOffer.tryParse(msg.text);

    return Align(
      alignment: isOut ? Alignment.centerRight : Alignment.centerLeft,
      child: Container(
        margin: const EdgeInsets.symmetric(vertical: 3),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
        constraints: BoxConstraints(maxWidth: MediaQuery.of(context).size.width * 0.72),
        decoration: BoxDecoration(
          // свои — контуром цвета акцента на фоне чата (на заливке серые и оранжевые галочки плохо видны), чужие — заливкой
          // обе стороны — контуром на фоне чата: свои зелёным, чужие #ff4001
          color: AppTheme.bg,
          border: Border.all(color: isOut ? AppTheme.accent.withOpacity(0.85) : AppTheme.incoming, width: 1.5),
          borderRadius: BorderRadius.only(
            topLeft:     const Radius.circular(16),
            topRight:    const Radius.circular(16),
            bottomLeft:  Radius.circular(isOut ? 16 : 4),
            bottomRight: Radius.circular(isOut ? 4  : 16),
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            if (msg.replyText != null)
              Align(alignment: Alignment.centerLeft, child: QuoteBox(author: quoteAuthor, text: msg.replyText!, onTap: onQuoteTap)),
            if (offer != null)
              _FileCard(offer: offer, outgoing: isOut, failed: msg.status == MessageStatus.failed)
            else if (GroupInviteMsg.tryParse(msg.text) case final inv?)
              _InviteCard(invite: inv, outgoing: isOut)
            else
              Text(msg.text, style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            const SizedBox(height: 2),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (edited)
                  const Padding(
                    padding: EdgeInsets.only(right: 4),
                    child: Text('изменено', style: TextStyle(color: AppTheme.textSecondary, fontSize: 10, fontStyle: FontStyle.italic)),
                  ),
                Text(time, style: const TextStyle(
                    color: AppTheme.textSecondary, fontSize: 10)),
                if (isOut) ...[
                  const SizedBox(width: 4),
                  Icon(_statusIcon(msg.status), size: 13, color: _statusColor(msg.status)),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }

  IconData _statusIcon(MessageStatus s) => switch (s) {
    MessageStatus.pending   => Icons.check,        // 1 серая — отправлено
    MessageStatus.delivered => Icons.done_all,     // 2 серые — доставлено
    MessageStatus.read      => Icons.done_all,     // 2 оранжевые — прочитано
    MessageStatus.failed    => Icons.error_outline,
  };

  Color _statusColor(MessageStatus s) => switch (s) {
    MessageStatus.read   => AppTheme.incoming,        // #ff4001: цвет рамки собеседника — «он это увидел»
    MessageStatus.failed => Colors.redAccent,
    _                    => AppTheme.textSecondary,   // серый: отправлено / доставлено
  };
}

class _InputBar extends StatelessWidget {
  final TextEditingController controller;
  final FocusNode focusNode;
  final bool ownKeyboard;
  final VoidCallback onSend;
  final VoidCallback onAttach;
  final bool recording;
  final int recordSeconds;
  final VoidCallback onMic, onRecordSend, onRecordCancel;
  const _InputBar({
    required this.controller, required this.focusNode, required this.ownKeyboard, required this.onSend, required this.onAttach,
    required this.recording, required this.recordSeconds, required this.onMic, required this.onRecordSend, required this.onRecordCancel,
  });

  static String _mmss(int s) => '${s ~/ 60}:${(s % 60).toString().padLeft(2, '0')}';

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppTheme.surface,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
      child: SafeArea(
        top: false,
        child: recording
            ? Row(children: [
                IconButton(
                  key: const ValueKey('voice-cancel'),
                  tooltip: 'Отменить запись',
                  icon: const Icon(Icons.delete_outline, color: Colors.redAccent),
                  onPressed: onRecordCancel,
                ),
                const Icon(Icons.fiber_manual_record, color: Colors.redAccent, size: 14),
                const SizedBox(width: 8),
                Text(_mmss(recordSeconds), key: const ValueKey('voice-timer'), style: const TextStyle(color: AppTheme.text, fontSize: 16)),
                const SizedBox(width: 12),
                const Expanded(child: Text('Идёт запись…', style: TextStyle(color: AppTheme.textSecondary))),
                GestureDetector(
                  key: const ValueKey('voice-send'),
                  onTap: onRecordSend,
                  child: Container(
                    width: 44, height: 44,
                    decoration: const BoxDecoration(shape: BoxShape.circle, color: AppTheme.accent),
                    child: const Icon(Icons.send, color: Colors.white, size: 20),
                  ),
                ),
              ])
            : Row(
          children: [
            IconButton(
              tooltip: 'Отправить файл',
              icon: const Icon(Icons.attach_file, color: AppTheme.textSecondary),
              onPressed: onAttach,
            ),
            Expanded(
              child: TextField(
                controller: controller,
                focusNode: focusNode,
                style: const TextStyle(color: AppTheme.text),
                maxLines: null,
                // со своей клавиатурой система к вводу не подключается вовсе: ни подсказок, ни обучения на нажатиях, ни облака
                keyboardType: ownKeyboard ? SecureKeyboard.inputType : null,
                enableSuggestions: !ownKeyboard,
                autocorrect: !ownKeyboard,
                enableIMEPersonalizedLearning: !ownKeyboard,
                textCapitalization: TextCapitalization.sentences,
                decoration: InputDecoration(
                  hintText: 'Сообщение...',
                  hintStyle: const TextStyle(color: AppTheme.textSecondary),
                  filled: true,
                  fillColor: AppTheme.bg,
                  border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(24),
                    borderSide: BorderSide.none,
                  ),
                  contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                ),
                onSubmitted: (_) => onSend(),
              ),
            ),
            const SizedBox(width: 8),
            // как в других мессенджерах: пока строка пуста — микрофон, с текстом — «отправить»
            ValueListenableBuilder<TextEditingValue>(
              valueListenable: controller,
              builder: (_, v, __) {
                final empty = v.text.trim().isEmpty;
                return GestureDetector(
                  key: ValueKey(empty ? 'voice-mic' : 'send-text'),
                  onTap: empty ? onMic : onSend,
                  child: Container(
                    width: 44, height: 44,
                    decoration: const BoxDecoration(
                      shape: BoxShape.circle,
                      color: AppTheme.accent,
                    ),
                    child: Icon(empty ? Icons.mic : Icons.send, color: Colors.white, size: 22),
                  ),
                );
              },
            ),
          ],
        ),
      ),
    );
  }
}


/// Карточка файла в чате: имя, размер, превью картинки, ход передачи и действия.
class _FileCard extends StatelessWidget {
  final FileOffer offer;
  final bool outgoing;
  final bool failed;
  const _FileCard({required this.offer, required this.outgoing, required this.failed});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final fs    = state.fileStateOf(offer.tid);
    final local = state.localPathFor(offer);
    final busy  = fs != null && !fs.failed;
    final error = fs?.failed == true ? fs!.error : null;
    final showImage = offer.isImage && local != null && File(local).existsSync();
    final inlineOk = local != null && !failed && !busy && error == null;
    final inlineAudio = inlineOk && offer.mime.startsWith('audio/') && offer.size <= InlineAudio.maxInline;
    final inlineVideo = inlineOk && offer.mime.startsWith('video/');

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (showImage)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(10),
              child: VaultImage(path: local, width: 220, cacheWidth: 440),
            ),
          ),
        if (!inlineAudio) Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(_icon(offer.mime), color: AppTheme.text, size: 28),
            const SizedBox(width: 10),
            Flexible(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(offer.name.startsWith('Голосовое_') ? 'Голосовое сообщение' : offer.name, maxLines: 2, overflow: TextOverflow.ellipsis,
                      style: const TextStyle(color: AppTheme.text, fontSize: 15, fontWeight: FontWeight.w600)),
                  Text(FileOffer.humanSize(offer.size) + ' · зашифровано',
                      style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: 8),
        // как в Telegram: звук и видео играют прямо в сообщении, без экрана «Открыть»
        if (inlineAudio) ...[
          InlineAudio(key: ValueKey('ia-${offer.tid}'), path: local, size: offer.size,
              caption: offer.name.startsWith('Голосовое_') ? null : offer.name),
          const SizedBox(height: 4),
        ] else if (inlineVideo) ...[
          InlineVideo(
            key: ValueKey('iv-${offer.tid}'),
            path: local,
            size: offer.size,
            mime: offer.mime,
            onFullscreen: () async {
              final v = await (await state.vaultFiles()).describe(local);
              if (!context.mounted) return;
              await Navigator.push(context, MaterialPageRoute(builder: (_) => FileViewerScreen(file: v)));
            },
          ),
          const SizedBox(height: 4),
        ],
        if (busy) ...[
          LinearProgressIndicator(value: fs.progress == 0 ? null : fs.progress,
              backgroundColor: Colors.black26, color: AppTheme.text),
          const SizedBox(height: 4),
          Text(outgoing ? 'Отправка ${(fs.progress * 100).round()}%' : 'Скачивание ${(fs.progress * 100).round()}%',
              style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
        ] else if (error != null) ...[
          Text(error, style: const TextStyle(color: Colors.redAccent, fontSize: 12)),
          if (!outgoing)
            _Action(icon: Icons.refresh, label: 'Повторить', onTap: () => state.downloadFile(offer)),
        ] else if (outgoing) ...[
          Text(failed ? 'Не отправлено' : 'Отправлено',
              style: TextStyle(color: failed ? Colors.redAccent : AppTheme.textSecondary, fontSize: 11)),
          if (local != null && !failed && !inlineAudio && !inlineVideo) Wrap(children: _openActions(context, state, local)),
        ] else if (local != null) ...[
          // у звука и видео своё воспроизведение прямо в сообщении: «Открыть» лишнее
          if (!inlineAudio && !inlineVideo) Wrap(children: _openActions(context, state, local)),
        ] else ...[
          _Action(icon: Icons.download, label: 'Скачать', onTap: () => state.downloadFile(offer)),
        ],
      ],
    );
  }

  List<Widget> _openActions(BuildContext context, AppState state, String path) => [
    // Просмотр — внутри приложения (файл не передаётся другим программам).
    _Action(icon: Icons.open_in_new, label: 'Открыть', onTap: () async {
      final v = await (await state.vaultFiles()).describe(path);
      if (!context.mounted) return;
      await Navigator.push(context, MaterialPageRoute(builder: (_) => FileViewerScreen(file: v)));
    }),
  ];

  IconData _icon(String mime) {
    if (mime.startsWith('image/')) return Icons.image_outlined;
    if (mime.startsWith('video/')) return Icons.movie_outlined;
    if (mime.startsWith('audio/')) return Icons.audiotrack_outlined;
    if (mime == 'application/pdf') return Icons.picture_as_pdf_outlined;
    if (mime == 'application/zip') return Icons.folder_zip_outlined;
    return Icons.insert_drive_file_outlined;
  }
}

class _Action extends StatelessWidget {
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  const _Action({required this.icon, required this.label, required this.onTap});

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(right: 8, top: 2),
    child: OutlinedButton.icon(
      onPressed: onTap,
      icon: Icon(icon, size: 16, color: AppTheme.text),
      label: Text(label, style: const TextStyle(color: AppTheme.text, fontSize: 12)),
      style: OutlinedButton.styleFrom(
        side: const BorderSide(color: Colors.white38),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 0),
        minimumSize: const Size(0, 32),
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      ),
    ),
  );
}

/// Приглашение в группу в чате: у получателя — кнопка «Вступить», у отправителя — пометка.
class _InviteCard extends StatelessWidget {
  final GroupInviteMsg invite;
  final bool outgoing;
  const _InviteCard({required this.invite, required this.outgoing});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final gid = invite.invite.gid;
    final inGroup = state.groups?.byId(gid) != null;
    Widget status(String text, {bool openBtn = false}) => Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
          Text(text, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          if (openBtn)
            TextButton(
              style: TextButton.styleFrom(padding: EdgeInsets.zero, minimumSize: const Size(0, 28), alignment: Alignment.centerLeft),
              onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => GroupChatScreen(gid: gid))),
              child: const Text('Открыть группу', style: TextStyle(color: AppTheme.accent)),
            ),
        ]);
    return Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
      Row(mainAxisSize: MainAxisSize.min, children: [
        Icon(Icons.group_add_outlined, color: inGroup && !outgoing ? AppTheme.textSecondary : AppTheme.accent),
        const SizedBox(width: 8),
        Flexible(child: Text('Приглашение в группу${invite.name.isEmpty ? '' : ' «${invite.name}»'}', style: const TextStyle(color: AppTheme.text, fontSize: 15))),
      ]),
      const SizedBox(height: 6),
      if (outgoing)
        const Text('Приглашение отправлено (действует сутки, один раз)', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12))
      else if (inGroup)
        status('Вы уже в этой группе', openBtn: true)
      else
        FutureBuilder<bool>(
          future: state.groups?.inviteSpent(gid, invite.invite.code) ?? Future.value(false),
          builder: (_, snap) => snap.data == true
              ? status('Приглашение уже недействительно')
              : FilledButton(
                  style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black, visualDensity: VisualDensity.compact),
                  onPressed: () => joinInvited(context, invite),
                  child: const Text('Вступить'),
                ),
        ),
    ]);
  }
}
