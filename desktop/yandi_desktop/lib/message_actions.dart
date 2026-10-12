import 'package:flutter/material.dart';
import 'preferences.dart';
import 'theme.dart';

/// Message commands are plain words in a menu, not buttons.
const commandEdit = 'Изменить',
    commandReply = 'Ответить',
    commandCopy = 'Копировать',
    commandDelete = 'Удалить',
    commandHideAuthor = 'Скрыть сообщения этого пользователя';

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
