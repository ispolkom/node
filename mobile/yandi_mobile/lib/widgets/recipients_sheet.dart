import 'package:flutter/material.dart';
import '../models/contact.dart';
import '../theme.dart';

/// Контакты, которым можно что-то переслать: сохранённые и недавно активные устройства; в сети — выше.
List<Contact> forwardCandidates(List<Contact> all, {String? exclude}) {
  final now = DateTime.now().millisecondsSinceEpoch;
  return all.where((c) => c.peerId != exclude && (c.saved || !c.isStale(now))).toList()
    ..sort((a, b) => a.saved != b.saved ? (a.saved ? -1 : 1) : a.online != b.online ? (a.online ? -1 : 1) : a.displayName.compareTo(b.displayName));
}

/// Список контактов с прокруткой, отметить можно до [max] получателей за раз.
Future<List<Contact>?> pickRecipients(BuildContext context, {required String title, required List<Contact> contacts, int max = 5}) {
  final chosen = <String>{};
  return showModalBottomSheet<List<Contact>>(
    context: context,
    backgroundColor: AppTheme.surface,
    isScrollControlled: true,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setSheet) => SafeArea(
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * 0.75),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
              child: Text(title, style: const TextStyle(color: AppTheme.text, fontSize: 16)),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text('Выберите до $max получателей. Уходит зашифрованным, ход виден в чатах с ними.',
                  style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ),
            Flexible(
              child: contacts.isEmpty
                  ? const Padding(
                      padding: EdgeInsets.all(24),
                      child: Text('Некому пересылать: в списке нет контактов', style: TextStyle(color: AppTheme.textSecondary)),
                    )
                  : ListView(shrinkWrap: true, children: [
                      for (final c in contacts)
                        CheckboxListTile(
                          key: ValueKey('rcpt-${c.peerId}'),
                          value: chosen.contains(c.peerId),
                          activeColor: AppTheme.accent,
                          controlAffinity: ListTileControlAffinity.trailing,
                          secondary: Icon(Icons.person_outline, color: c.online ? AppTheme.accent : AppTheme.textSecondary),
                          title: Text(c.displayName, style: const TextStyle(color: AppTheme.text)),
                          subtitle: Text(c.online ? 'в сети' : 'не в сети', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                          onChanged: (v) {
                            if (v == true && chosen.length >= max) {
                              ScaffoldMessenger.of(ctx).showSnackBar(SnackBar(content: Text('Не больше $max получателей за раз')));
                              return;
                            }
                            setSheet(() => v == true ? chosen.add(c.peerId) : chosen.remove(c.peerId));
                          },
                        ),
                    ]),
            ),
            Padding(
              padding: const EdgeInsets.all(12),
              child: SizedBox(
                width: double.infinity,
                child: FilledButton(
                  key: const ValueKey('rcpt-send'),
                  style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                  onPressed: chosen.isEmpty ? null : () => Navigator.pop(ctx, contacts.where((c) => chosen.contains(c.peerId)).toList()),
                  child: Text(chosen.isEmpty ? 'Выберите получателей' : 'Отправить (${chosen.length})'),
                ),
              ),
            ),
          ]),
        ),
      ),
    ),
  );
}
