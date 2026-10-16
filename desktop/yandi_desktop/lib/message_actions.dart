import 'dart:convert';
import 'package:flutter/material.dart';
import 'domain/shell_repository.dart';
import 'preferences.dart';
import 'theme.dart';

/// Message commands are plain words in a menu, not buttons.
const commandEdit = 'Изменить',
    commandReply = 'Ответить',
    commandCopy = 'Копировать',
    commandDelete = 'Удалить',
    commandHideAuthor = 'Скрыть сообщения этого пользователя',
    commandSelect = 'Выбрать',
    commandShare = 'Переслать',
    commandThread = 'Вся цепочка ответов',
    commandMute = 'Заглушить',
    commandReact = 'Смайл…',
    commandApprove = '🪙 Одобряю',
    commandDisapprove = '🪙 Не одобряю';

Future<String?> showMessageMenu(
  BuildContext context,
  Offset at,
  List<String> commands,
) {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
  return showMenu<String>(
    context: context,
    color: Icq.light,
    elevation: 2,
    shape: RoundedRectangleBorder(
      side: const BorderSide(color: Icq.border),
      borderRadius: BorderRadius.circular(2),
    ),
    position: RelativeRect.fromRect(
      at & const Size(1, 1),
      Offset.zero & overlay.size,
    ),
    items: [
      for (final c in commands)
        PopupMenuItem<String>(
          value: c,
          height: 34,
          child: UiText(
            c,
            style: TextStyle(color: c == commandDelete ? Icq.danger : Icq.text),
          ),
        ),
    ],
  );
}

/// A word that acts as a command (no button chrome).
class CommandText extends StatelessWidget {
  final String text;
  final VoidCallback? onTap;
  const CommandText(this.text, {super.key, this.onTap});
  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap,
    child: Padding(
      padding: const EdgeInsets.symmetric(vertical: 2, horizontal: 2),
      child: UiText(
        text,
        style: TextStyle(
          fontSize: 12,
          color: onTap == null ? Icq.dark : Icq.header,
          decoration: TextDecoration.underline,
        ),
      ),
    ),
  );
}

/// Кнопка-иконка с подсказкой: подсказка появляется, если держать мышь над кнопкой 2 секунды.
class IconAction extends StatelessWidget {
  final IconData icon;
  final String tip;
  final VoidCallback? onTap;
  final bool active;
  final Color? color;
  const IconAction({super.key, required this.icon, required this.tip, this.onTap, this.active = false, this.color});
  @override
  Widget build(BuildContext context) => Tooltip(
    message: tr(context, tip),
    waitDuration: const Duration(seconds: 2),
    child: InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(4),
      child: Container(
        padding: const EdgeInsets.all(7),
        decoration: BoxDecoration(
          color: active ? Icq.active.withValues(alpha: 0.18) : null,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Icon(icon, size: 22, color: onTap == null ? Icq.dark : (color ?? Icq.header)),
      ),
    ),
  );
}

/// The quoted message above a reply, or above the composer while replying.
class QuoteCard extends StatelessWidget {
  final String? author;
  final String text;
  final VoidCallback? onTap;
  final Widget? trailing;
  const QuoteCard({
    super.key,
    this.author,
    required this.text,
    this.onTap,
    this.trailing,
  });
  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap,
    child: Container(
      margin: const EdgeInsets.only(bottom: 4),
      padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
      decoration: const BoxDecoration(
        color: Icq.panel,
        border: Border(left: BorderSide(color: Icq.header, width: 3)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (author != null && author!.isNotEmpty)
                  Text(
                    author!,
                    style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.bold,
                      color: Icq.header,
                    ),
                  ),
                Text(
                  text,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 12, color: Icq.text),
                ),
              ],
            ),
          ),
          if (trailing != null) trailing!,
        ],
      ),
    ),
  );
}

String quoteExcerpt(Object? text) {
  final t = '${text ?? ''}'.replaceAll(RegExp(r'\s+'), ' ').trim();
  return t.runes.length <= 200 ? t : String.fromCharCodes(t.runes.take(200));
}

/// Time and delivery state as in the phone app: one tick sent, two delivered,
/// two orange read, cross failed. The word status stays as a tooltip/semantics.
class DeliveryMark extends StatelessWidget {
  final String time;
  final dynamic status;
  final bool own;
  final bool edited;
  const DeliveryMark({
    super.key,
    required this.time,
    required this.status,
    required this.own,
    this.edited = false,
  });
  @override
  Widget build(BuildContext context) {
    final word = tr(context, ShellRepository.delivery(status));
    final (glyph, color) = switch (status) {
      'Read' => ('✓✓', Icq.peer),
      'Delivered' => ('✓✓', Icq.dark),
      'Sent' => ('✓', Icq.dark),
      'Failed' => ('✗', const Color(0xFFC62828)),
      _ => ('…', Icq.dark),
    };
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (edited)
          UiText(
            'изменено',
            style: const TextStyle(
              fontSize: 11,
              color: Icq.dark,
              fontStyle: FontStyle.italic,
            ),
          ),
        if (edited) const SizedBox(width: 6),
        Text(time, style: const TextStyle(fontSize: 11, color: Icq.dark)),
        if (own) ...[
          const SizedBox(width: 4),
          Tooltip(
            message: word,
            child: Semantics(
              label: word,
              child: Text(
                glyph,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.bold,
                  color: color,
                ),
              ),
            ),
          ),
        ],
      ],
    );
  }
}

/// Group invitation sent in a private chat by the phone app (see CLIENT_WIRE).
class GroupInviteMessage {
  static const marker = '\u0001yandi-gi:';
  final String gid, code, name;
  const GroupInviteMessage(this.gid, this.code, this.name);
  static GroupInviteMessage? tryParse(String text) {
    if (!text.startsWith(marker)) return null;
    try {
      final j = jsonDecode(text.substring(marker.length));
      if (j is! Map) return null;
      final gid = '${j['g'] ?? ''}'.toLowerCase(), code = '${j['c'] ?? ''}';
      if (!RegExp(r'^[0-9a-f]{32}$').hasMatch(gid)) return null;
      if (code.isNotEmpty &&
          !RegExp(r'^[0-9A-Za-z_-]{8,128}$').hasMatch(code)) {
        return null;
      }
      final n = '${j['n'] ?? ''}'.replaceAll(RegExp(r'[\x00-\x1f]'), '').trim();
      return GroupInviteMessage(
        gid,
        code,
        n.length > 64 ? n.substring(0, 64) : n,
      );
    } catch (_) {
      return null;
    }
  }
}

class GroupInviteCard extends StatelessWidget {
  final GroupInviteMessage invite;
  final bool outgoing;
  final VoidCallback? onJoin;
  const GroupInviteCard({
    super.key,
    required this.invite,
    required this.outgoing,
    this.onJoin,
  });
  @override
  Widget build(BuildContext context) => Container(
    margin: const EdgeInsets.symmetric(vertical: 4),
    padding: const EdgeInsets.all(8),
    decoration: BoxDecoration(
      color: const Color(0x14000000),
      border: Border.all(color: Icq.dark.withValues(alpha: 0.35)),
    ),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.group_add_outlined, color: Icq.header),
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                '${tr(context, 'Приглашение в группу')}${invite.name.isEmpty ? '' : ' «${invite.name}»'}',
                style: const TextStyle(fontWeight: FontWeight.bold),
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        if (outgoing)
          const UiText(
            'Приглашение отправлено (действует сутки, один раз)',
            style: TextStyle(fontSize: 12, color: Icq.dark),
          )
        else
          CommandText('Вступить в группу', onTap: onJoin),
      ],
    ),
  );
}

/// Round initial badge next to authors, as the phone app's avatar circle.
class InitialBadge extends StatelessWidget {
  final String name;
  final double size;
  const InitialBadge(this.name, {super.key, this.size = 22});
  @override
  Widget build(BuildContext context) {
    final t = name.trim();
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: const BoxDecoration(
        color: Icq.header,
        shape: BoxShape.circle,
      ),
      child: Text(
        t.isEmpty ? '?' : t.characters.first.toUpperCase(),
        style: TextStyle(
          color: Colors.white,
          fontSize: size * 0.55,
          fontWeight: FontWeight.bold,
          fontFamily: 'Tahoma',
        ),
      ),
    );
  }
}

/// Same set as the phone app's own keyboard panel.
const emojiPanel = [
  '😀',
  '😂',
  '🥹',
  '😊',
  '😍',
  '🥰',
  '😘',
  '😎',
  '🤔',
  '😅',
  '😭',
  '😡',
  '🥳',
  '😴',
  '🤯',
  '😱',
  '👍',
  '👎',
  '👏',
  '🙏',
  '💪',
  '👋',
  '🤝',
  '✌️',
  '👌',
  '🤞',
  '🫡',
  '🙌',
  '❤️',
  '💔',
  '🔥',
  '✨',
  '🎉',
  '🎁',
  '☀️',
  '🌙',
  '⭐',
  '🌈',
  '🍀',
  '🌸',
  '🍕',
  '☕',
  '🍺',
  '🎂',
  '🚗',
  '✈️',
  '🏠',
  '📞',
  '🐱',
  '🐶',
  '🦊',
  '🐼',
  '🦁',
  '🐸',
  '🐵',
  '🦉',
  '🐧',
  '🐢',
  '🦄',
  '🐙',
  '🔑',
  '🛡️',
  '💎',
  '✅',
];

/// Paperclip and emoji icons of the input row, as in the phone app.
class ComposerTools extends StatelessWidget {
  final VoidCallback? onAttach;
  final TextEditingController controller;
  const ComposerTools({super.key, this.onAttach, required this.controller});
  @override
  Widget build(BuildContext context) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      IconButton(
        tooltip: tr(context, 'Выбрать файл'),
        onPressed: onAttach,
        icon: const Icon(Icons.attach_file, color: Icq.header),
      ),
      IconButton(
        tooltip: tr(context, 'Эмодзи'),
        icon: const Icon(Icons.emoji_emotions_outlined, color: Icq.header),
        onPressed: () async {
          final e = await showDialog<String>(
            context: context,
            builder: (ctx) => AlertDialog(
              title: UiText('Эмодзи'),
              content: SizedBox(
                width: 360,
                child: Wrap(
                  children: [
                    for (final e in emojiPanel)
                      InkWell(
                        onTap: () => Navigator.pop(ctx, e),
                        child: SizedBox(
                          width: 40,
                          height: 40,
                          child: Center(
                            child: Text(
                              e,
                              style: const TextStyle(
                                fontSize: 22,
                                fontFamily: 'NotoColorEmoji',
                              ),
                            ),
                          ),
                        ),
                      ),
                  ],
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
          if (e == null) return;
          final v = controller.value;
          final sel = v.selection;
          final at = sel.isValid ? sel.start : v.text.length;
          final end = sel.isValid ? sel.end : v.text.length;
          controller.value = TextEditingValue(
            text: v.text.replaceRange(at, end, e),
            selection: TextSelection.collapsed(offset: at + e.length),
          );
        },
      ),
    ],
  );
}

/// Выбор получателей пересылки: до [max] за раз (как в телефоне), слова-команды вместо кнопок.
/// Возвращает выбранные контакты или null, если отказались.
Future<List<Map<String, dynamic>>?> pickRecipients(
  BuildContext context, {
  required String title,
  required List<Map<String, dynamic>> contacts,
  int max = 5,
}) => showDialog<List<Map<String, dynamic>>>(
  context: context,
  builder: (_) => _RecipientsDialog(title: title, contacts: contacts, max: max),
);

class _RecipientsDialog extends StatefulWidget {
  final String title;
  final List<Map<String, dynamic>> contacts;
  final int max;
  const _RecipientsDialog({
    required this.title,
    required this.contacts,
    required this.max,
  });
  @override
  State<_RecipientsDialog> createState() => _RecipientsDialogState();
}

class _RecipientsDialogState extends State<_RecipientsDialog> {
  final chosen = <int>{};
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: UiText(widget.title),
    content: SizedBox(
      width: 360,
      child: widget.contacts.isEmpty
          ? const UiText('Некому отправлять: в списке нет контактов.')
          : SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  UiText(
                    'Выбрано: ${chosen.length} из ${widget.max}',
                    style: const TextStyle(fontSize: 12, color: Icq.dark),
                  ),
                  for (var i = 0; i < widget.contacts.length; i++)
                    InkWell(
                      key: ValueKey('recipient-$i'),
                      onTap: () => setState(() {
                        if (!chosen.remove(i) && chosen.length < widget.max) {
                          chosen.add(i);
                        }
                      }),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 5),
                        child: Row(
                          children: [
                            Text(
                              chosen.contains(i) ? '☑' : '☐',
                              style: const TextStyle(
                                fontSize: 16,
                                color: Icq.header,
                              ),
                            ),
                            const SizedBox(width: 8),
                            Flexible(
                              child: Text('${widget.contacts[i]['name']}'),
                            ),
                          ],
                        ),
                      ),
                    ),
                ],
              ),
            ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: chosen.isEmpty
            ? null
            : () => Navigator.pop(context, [
                for (final i in chosen.toList()..sort()) widget.contacts[i],
              ]),
        child: const UiText('Переслать'),
      ),
    ],
  );
}

/// Самолётик, пока в строке есть текст, и микрофон, пока строка пуста — как в телефоне.
class SendOrMic extends StatelessWidget {
  final TextEditingController controller;
  final VoidCallback? onSend;
  final VoidCallback? onMic;
  final String sendTip;
  final bool hasFile;
  const SendOrMic({
    super.key,
    required this.controller,
    required this.onSend,
    required this.onMic,
    required this.sendTip,
    this.hasFile = false,
  });
  @override
  Widget build(BuildContext context) =>
      ValueListenableBuilder<TextEditingValue>(
        valueListenable: controller,
        builder: (_, v, _) {
          final empty = v.text.trim().isEmpty && !hasFile;
          return empty
              ? IconButton(
                  key: const ValueKey('voice-mic'),
                  tooltip: tr(context, 'Голосовое сообщение'),
                  onPressed: onMic,
                  icon: const Icon(Icons.mic, color: Icq.header),
                )
              : IconButton(
                  key: const ValueKey('send-text'),
                  tooltip: tr(context, sendTip),
                  onPressed: onSend,
                  icon: const Icon(Icons.send, color: Icq.header),
                );
        },
      );
}

/// Строка во время записи голосового: время, «Отмена» и самолётик для отправки.
class VoiceBar extends StatelessWidget {
  final int seconds;
  final VoidCallback onCancel;
  final VoidCallback onSend;
  const VoiceBar({
    super.key,
    required this.seconds,
    required this.onCancel,
    required this.onSend,
  });
  @override
  Widget build(BuildContext context) {
    final t = '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
    return Row(
      children: [
        const Icon(Icons.fiber_manual_record, color: Icq.danger, size: 14),
        const SizedBox(width: 8),
        Text(t, style: const TextStyle(fontSize: 16)),
        const SizedBox(width: 12),
        const Expanded(child: UiText('Идёт запись…')),
        CommandText('Отмена', onTap: onCancel),
        IconButton(
          key: const ValueKey('voice-send'),
          tooltip: tr(context, 'Отправить голосовое'),
          onPressed: onSend,
          icon: const Icon(Icons.send, color: Icq.header),
        ),
      ],
    );
  }
}
