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
    commandSelect = 'Выбрать';

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
      if (code.isNotEmpty && !RegExp(r'^[0-9A-Za-z_-]{8,128}$').hasMatch(code)) {
        return null;
      }
      final n = '${j['n'] ?? ''}'.replaceAll(RegExp(r'[\x00-\x1f]'), '').trim();
      return GroupInviteMessage(gid, code, n.length > 64 ? n.substring(0, 64) : n);
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
      decoration: const BoxDecoration(color: Icq.header, shape: BoxShape.circle),
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
