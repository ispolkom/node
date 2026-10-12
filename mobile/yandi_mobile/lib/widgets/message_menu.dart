import 'package:flutter/material.dart';
import '../theme.dart';

/// Команды сообщения — слова в меню, а не кнопки.
const cmdEdit = 'Изменить',
    cmdReply = 'Ответить',
    cmdCopy = 'Копировать',
    cmdShare = 'Переслать',
    cmdSelect = 'Выбрать',
    cmdDelete = 'Удалить',
    cmdHideAuthor = 'Скрыть сообщения этого пользователя',
    cmdEraseAll = 'Стереть у всех',
    cmdMute = 'Заглушить';

Future<String?> showMessageMenu(
  BuildContext context,
  Offset at,
  List<String> commands,
) {
  final size = MediaQuery.of(context).size;
  return showMenu<String>(
    context: context,
    color: AppTheme.surface,
    position: RelativeRect.fromLTRB(
      at.dx,
      at.dy,
      size.width - at.dx,
      size.height - at.dy,
    ),
    items: [
      for (final c in commands)
        PopupMenuItem<String>(
          value: c,
          height: 44,
          child: Text(
            c,
            style: TextStyle(
              color: c == cmdDelete || c == cmdEraseAll
                  ? Colors.redAccent
                  : AppTheme.text,
              fontSize: 15,
            ),
          ),
        ),
    ],
  );
}

/// Цитата сообщения, на которое отвечают: над ответом в ленте и над полем ввода.
class QuoteBox extends StatelessWidget {
  final String? author;
  final String text;
  final VoidCallback? onTap;
  final VoidCallback? onClose;
  const QuoteBox({
    super.key,
    this.author,
    required this.text,
    this.onTap,
    this.onClose,
  });

  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap,
    child: Container(
      margin: const EdgeInsets.only(bottom: 4),
      padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
      decoration: BoxDecoration(
        color: AppTheme.accent.withOpacity(0.08),
        border: const Border(
          left: BorderSide(color: AppTheme.accent, width: 3),
        ),
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
                      color: AppTheme.accent,
                      fontSize: 12,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                Text(
                  text,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(color: AppTheme.text, fontSize: 13),
                ),
              ],
            ),
          ),
          if (onClose != null)
            GestureDetector(
              onTap: onClose,
              child: const Padding(
                padding: EdgeInsets.only(left: 8),
                child: Icon(
                  Icons.close,
                  size: 18,
                  color: AppTheme.textSecondary,
                ),
              ),
            ),
        ],
      ),
    ),
  );
}

/// Куда поделиться: контакт или группа. Пересылка остаётся внутри YANDI (шифруется как обычное сообщение), а не уходит в чужие приложения.
class ShareTarget {
  final bool group;
  final String id, title;
  const ShareTarget.contact(this.id, this.title) : group = false;
  const ShareTarget.group(this.id, this.title) : group = true;
}

Future<ShareTarget?> pickShareTarget(BuildContext context, List<ShareTarget> targets) {
  return showModalBottomSheet<ShareTarget>(
    context: context,
    backgroundColor: AppTheme.surface,
    isScrollControlled: true,
    builder: (ctx) => SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * 0.6),
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          const Padding(
            padding: EdgeInsets.all(14),
            child: Text('Поделиться с…', style: TextStyle(color: AppTheme.text, fontSize: 16)),
          ),
          Flexible(
            child: targets.isEmpty
                ? const Padding(
                    padding: EdgeInsets.all(20),
                    child: Text('Некому пересылать', style: TextStyle(color: AppTheme.textSecondary)),
                  )
                : ListView(shrinkWrap: true, children: [
                    for (final t in targets)
                      ListTile(
                        leading: Icon(t.group ? Icons.group_outlined : Icons.person_outline, color: AppTheme.accent),
                        title: Text(t.title, style: const TextStyle(color: AppTheme.text)),
                        onTap: () => Navigator.pop(ctx, t),
                      ),
                  ]),
          ),
        ]),
      ),
    ),
  );
}
