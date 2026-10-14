import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';
import 'secure_text_field.dart';

const _reasons = <(String, String)>[
  ('spam', 'Спам или навязчивость'),
  ('abuse', 'Оскорбления, травля'),
  ('illegal', 'Запрещённое содержимое'),
  ('danger', 'Угрозы, опасность'),
  ('other', 'Другое'),
];

/// Окно «Пожаловаться» на пользователя или группу. Жалобу видит только владелец узла; на кого жалоба, не узнает, кто подал.
Future<void> showComplaintSheet(BuildContext context, {required String kind, required String targetId, required String title}) {
  final state = context.read<AppState>();
  final messenger = ScaffoldMessenger.of(context);
  final textCtrl = TextEditingController();
  String reason = 'spam';
  bool attach = true;
  bool busy = false;
  String? error;
  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    backgroundColor: AppTheme.surface,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setS) => Padding(
        padding: EdgeInsets.fromLTRB(16, 16, 16, 16 + MediaQuery.of(ctx).viewInsets.bottom),
        child: SingleChildScrollView(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
            Text('Пожаловаться: $title', style: const TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold)),
            const SizedBox(height: 4),
            const Text('Жалобу увидит только владелец узла. Тот, на кого жалоба, не узнает, что это вы.',
                style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            const SizedBox(height: 8),
            for (final r in _reasons)
              RadioListTile<String>(
                value: r.$1, groupValue: reason, dense: true, activeColor: AppTheme.accent,
                title: Text(r.$2, style: const TextStyle(color: AppTheme.text)),
                onChanged: busy ? null : (v) => setS(() => reason = v!),
              ),
            SecureTextField(
              controller: textCtrl, maxLength: 1000, maxLines: 3,
              style: const TextStyle(color: AppTheme.text),
              decoration: const InputDecoration(labelText: 'Что случилось', hintText: 'Опишите коротко'),
            ),
            if (kind == 'user')
              CheckboxListTile(
                value: attach, dense: true, activeColor: AppTheme.accent, controlAffinity: ListTileControlAffinity.leading,
                title: const Text('Приложить последние сообщения этого человека', style: TextStyle(color: AppTheme.text, fontSize: 13)),
                subtitle: const Text('Узел сообщений не читает: доказательства прикладываете вы', style: TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                onChanged: busy ? null : (v) => setS(() => attach = v ?? false),
              ),
            if (error != null) Padding(padding: const EdgeInsets.only(top: 4), child: Text(error!, style: const TextStyle(color: AppTheme.incoming))),
            const SizedBox(height: 8),
            Row(children: [
              Expanded(
                child: FilledButton(
                  style: FilledButton.styleFrom(backgroundColor: AppTheme.accent, foregroundColor: Colors.black),
                  onPressed: busy ? null : () async {
                    if (textCtrl.text.trim().isEmpty) { setS(() => error = 'Опишите, что случилось'); return; }
                    setS(() { busy = true; error = null; });
                    final err = await state.complain(kind: kind, targetId: targetId, reason: reason, text: textCtrl.text.trim(), attachRecent: attach);
                    if (!ctx.mounted) return;
                    if (err == null) {
                      Navigator.pop(ctx);
                      messenger.showSnackBar(const SnackBar(content: Text('Жалоба отправлена владельцу узла')));
                    } else {
                      setS(() { busy = false; error = err; });
                    }
                  },
                  child: const Text('Отправить'),
                ),
              ),
              const SizedBox(width: 8),
              TextButton(onPressed: busy ? null : () => Navigator.pop(ctx), child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary))),
            ]),
          ]),
        ),
      ),
    ),
  );
}
