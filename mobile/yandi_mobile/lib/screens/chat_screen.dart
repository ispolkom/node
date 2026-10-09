import 'dart:io';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:open_filex/open_filex.dart';
import 'package:intl/intl.dart';
import 'package:provider/provider.dart';
import '../models/file_offer.dart';
import '../models/message.dart';
import '../services/app_state.dart';
import '../services/file_transfer_service.dart' show FileTransferService, maxFileSize;
import '../theme.dart';
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

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final state = context.read<AppState>();
      state.activeChatPeerId = widget.peerId;
      state.loadChatHistory(widget.peerId);
    });
  }

  @override
  void dispose() {
    context.read<AppState>().activeChatPeerId = null;
    _inputCtrl.dispose();
    _scrollCtrl.dispose();
    super.dispose();
  }

  void _send() {
    final text = _inputCtrl.text.trim();
    if (text.isEmpty) return;
    _inputCtrl.clear();
    context.read<AppState>().sendMessage(widget.peerId, text);
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
  Future<void> _pickAndSend() async {
    final state = context.read<AppState>();
    if (!await state.canSendFiles(widget.peerId)) {
      _snack('Файлы можно отправлять только на свои устройства (у этого собеседника нет ключа шифрования)');
      return;
    }
    final res = await FilePicker.platform.pickFiles(allowMultiple: true);
    if (res == null) return;
    for (final f in res.files) {
      final path = f.path;
      if (path == null) continue;
      if (f.size > maxFileSize) {
        _snack('«${f.name}» больше 1 ГБ: слишком большой');
        continue;
      }
      final err = await state.sendFile(widget.peerId, path, name: f.name);
      if (err != null) _snack(err);
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

    WidgetsBinding.instance.addPostFrameCallback((_) => _scrollToBottom());

    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        elevation: 0,
        title: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, style: const TextStyle(color: AppTheme.text, fontSize: 16)),
            Text(widget.peerId.substring(0, 16) + '...',
                style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
          ],
        ),
        iconTheme: const IconThemeData(color: AppTheme.text),
        actions: [
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
                    itemBuilder: (_, i) => _MessageBubble(msg: messages[i]),
                  ),
          ),
          _InputBar(
            controller: _inputCtrl,
            onSend: _send,
            onAttach: _pickAndSend,
          ),
        ],
      ),
    );
  }
}

class _MessageBubble extends StatelessWidget {
  final ChatMessage msg;
  const _MessageBubble({required this.msg});

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
          color: isOut ? AppTheme.accent.withOpacity(0.85) : AppTheme.surface,
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
            if (offer != null)
              _FileCard(offer: offer, outgoing: isOut, failed: msg.status == MessageStatus.failed)
            else
              Text(msg.text, style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            const SizedBox(height: 2),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(time, style: const TextStyle(
                    color: AppTheme.textSecondary, fontSize: 10)),
                if (isOut) ...[
                  const SizedBox(width: 4),
                  Icon(_statusIcon(msg.status), size: 12, color: AppTheme.textSecondary),
                ],
              ],
            ),
          ],
        ),
      ),
    );
  }

  IconData _statusIcon(MessageStatus s) => switch (s) {
    MessageStatus.pending   => Icons.access_time,
    MessageStatus.delivered => Icons.done,
    MessageStatus.read      => Icons.done_all,
    MessageStatus.failed    => Icons.error_outline,
  };
}

class _InputBar extends StatelessWidget {
  final TextEditingController controller;
  final VoidCallback onSend;
  final VoidCallback onAttach;
  const _InputBar({required this.controller, required this.onSend, required this.onAttach});

  @override
  Widget build(BuildContext context) {
    return Container(
      color: AppTheme.surface,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
      child: SafeArea(
        top: false,
        child: Row(
          children: [
            IconButton(
              tooltip: 'Отправить файл',
              icon: const Icon(Icons.attach_file, color: AppTheme.textSecondary),
              onPressed: onAttach,
            ),
            Expanded(
              child: TextField(
                controller: controller,
                style: const TextStyle(color: AppTheme.text),
                maxLines: null,
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
            GestureDetector(
              onTap: onSend,
              child: Container(
                width: 44, height: 44,
                decoration: const BoxDecoration(
                  shape: BoxShape.circle,
                  color: AppTheme.accent,
                ),
                child: const Icon(Icons.send, color: Colors.white, size: 20),
              ),
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

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (showImage)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(10),
              child: Image.file(File(local), width: 220, cacheWidth: 440, fit: BoxFit.cover,
                  errorBuilder: (_, __, ___) => const SizedBox.shrink()),
            ),
          ),
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(_icon(offer.mime), color: AppTheme.text, size: 28),
            const SizedBox(width: 10),
            Flexible(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(offer.name, maxLines: 2, overflow: TextOverflow.ellipsis,
                      style: const TextStyle(color: AppTheme.text, fontSize: 15, fontWeight: FontWeight.w600)),
                  Text(FileOffer.humanSize(offer.size) + ' · зашифровано',
                      style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: 8),
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
          if (local != null && !failed) Wrap(children: _openActions(context, state, local)),
        ] else if (local != null) ...[
          Wrap(children: _openActions(context, state, local)),
        ] else ...[
          _Action(icon: Icons.download, label: 'Скачать', onTap: () => state.downloadFile(offer)),
        ],
      ],
    );
  }

  List<Widget> _openActions(BuildContext context, AppState state, String path) => [
    _Action(icon: Icons.open_in_new, label: 'Открыть', onTap: () async {
      final r = await OpenFilex.open(path, type: offer.mime == 'application/octet-stream' ? null : offer.mime);
      if (r.type != ResultType.done && context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('Нечем открыть этот файл (${r.message})')));
      }
    }),
    _Action(icon: Icons.save_alt, label: 'В «Загрузки»', onTap: () async {
      final uri = await state.saveToDownloads(path, FileTransferService.safeName(offer.name), offer.mime);
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
            content: Text(uri == null ? 'Не удалось сохранить в «Загрузки»' : 'Сохранено в «Загрузки»: ${offer.name}')));
      }
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
