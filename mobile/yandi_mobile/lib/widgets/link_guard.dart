import 'dart:io';
import 'package:flutter/material.dart';
import '../content_policy.dart';
import '../theme.dart';
import '../l10n.dart';

/// Если в тексте есть ссылка — объясняет, что ссылки в YANDI не передаются, и предлагает удалить её.
/// Возвращает текст, который можно отправлять (без ссылок), либо null, если отправитель отказался или после удаления пусто.
Future<String?> guardLinks(BuildContext context, String text) async {
  final link = findLink(text);
  if (link == null) return text;
  final cleaned = removeLinks(text);
  final go = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      backgroundColor: AppTheme.surface,
      title: Text(tr('Ссылки не передаются'), style: TextStyle(color: AppTheme.text)),
      content: Text(
        trf('В YANDI нельзя отправлять ссылки: сеть их не пропускает.\n\nНайдено: «{l}»', {'l': link.length > 60 ? '${link.substring(0, 60)}…' : link}),
        style: const TextStyle(color: AppTheme.textSecondary),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'), style: TextStyle(color: AppTheme.textSecondary))),
        TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Удалить ссылку'), style: TextStyle(color: AppTheme.accent))),
      ],
    ),
  );
  if (go != true || cleaned.isEmpty) return null;
  return cleaned;
}

/// Проверка вложения перед отправкой: имя по белому списку и первые байты содержимого. null — можно отправлять.
Future<String?> attachmentVerdict(String path, String name) async {
  final byName = attachmentProblem(name);
  if (byName != null) return byName;
  try {
    final raf = await File(path).open();
    try {
      final head = await raf.read(64);
      return contentProblem(name, head);
    } finally {
      await raf.close();
    }
  } catch (_) {
    return tr('Не удалось прочитать файл для проверки');
  }
}
