import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../l10n.dart';
import '../services/app_state.dart';
import '../theme.dart';

const Color _yellow = Color(0xFFFFC107);

/// Жёлтое окно в жёлтой рамке: закрытая гостю возможность не скрывается, а объясняется. [what] — что именно закрыто (необязательно).
Future<void> showLockedNotice(BuildContext context, {String? what}) {
  var pending = false;
  try { pending = context.read<AppState>().statusRequest == 'pending'; } catch (_) {}
  return showDialog<void>(
    context: context,
    builder: (ctx) => Dialog(
      backgroundColor: AppTheme.surface,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12), side: const BorderSide(color: _yellow, width: 2)),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
          Row(children: [
            const Icon(Icons.lock_outline, color: _yellow),
            const SizedBox(width: 8),
            Expanded(child: Text(tr('Доступно после полной авторизации'), key: const ValueKey('locked-title'), style: const TextStyle(color: _yellow, fontWeight: FontWeight.bold, fontSize: 16))),
          ]),
          const SizedBox(height: 12),
          if (what != null) Padding(padding: const EdgeInsets.only(bottom: 8), child: Text(what, key: const ValueKey('locked-what'), style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.w600))),
          Text(
            tr('Эта возможность открывается, когда администратор узла подтвердит ваш профиль. Запрос можно отправить в «Настройках». Если он вам не нужен, пользуйтесь тем, что доступно гостю.'),
            key: const ValueKey('locked-body'),
            style: const TextStyle(color: AppTheme.textSecondary, fontSize: 14, height: 1.35),
          ),
          const SizedBox(height: 12),
          Row(mainAxisAlignment: MainAxisAlignment.end, children: [
            TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Понятно'))),
            if (!pending)
              TextButton(
                key: const ValueKey('locked-request'),
                onPressed: () {
                  Navigator.pop(ctx);
                  showStatusRequestDialog(context);
                },
                child: Text(tr('Запросить статус'), style: const TextStyle(color: _yellow)),
              ),
          ]),
        ]),
      ),
    ),
  );
}

/// Окно «Запросить статус пользователя»: короткий текст администратору узла (он увидит «Гость Х: текст»).
Future<void> showStatusRequestDialog(BuildContext context) async {
  final state = context.read<AppState>();
  final ctrl = TextEditingController();
  String? error;
  await showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(builder: (ctx, setS) => AlertDialog(
      backgroundColor: AppTheme.surface,
      title: Text(tr('Запрос статуса пользователя'), style: const TextStyle(color: AppTheme.text)),
      content: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(tr('Напишите администратору узла, кто вы и почему просите статус. Администрация принимает решение на своё усмотрение. Ссылки в тексте не допускаются.'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
        const SizedBox(height: 8),
        TextField(
          key: const ValueKey('status-request-text'),
          controller: ctrl,
          maxLength: 300,
          maxLines: 4,
          style: const TextStyle(color: AppTheme.text),
          decoration: InputDecoration(hintText: tr('Прошу авторизовать мой профиль…')),
        ),
        if (error != null) Text(error!, key: const ValueKey('status-request-error'), style: const TextStyle(color: Colors.redAccent)),
      ]),
      actions: [
        TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Отмена'))),
        TextButton(
          key: const ValueKey('status-request-send'),
          onPressed: () async {
            final err = await state.requestUserStatus(ctrl.text.trim());
            if (err != null) { setS(() => error = err); return; }
            if (ctx.mounted) Navigator.pop(ctx);
          },
          child: Text(tr('Отправить')),
        ),
      ],
    )),
  );
  ctrl.dispose();
}

/// Вкладка целиком закрыта (например «Группы» у гостя): жёлтая рамка вместо содержимого, нажатие открывает пояснение.
class LockedPanel extends StatelessWidget {
  final String title;
  const LockedPanel({super.key, required this.title});
  @override
  Widget build(BuildContext context) => Center(
    child: Padding(
      padding: const EdgeInsets.all(20),
      child: InkWell(
        key: const ValueKey('locked-panel'),
        onTap: () => showLockedNotice(context, what: title),
        borderRadius: BorderRadius.circular(12),
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(color: AppTheme.surface, borderRadius: BorderRadius.circular(12), border: Border.all(color: _yellow, width: 2)),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Icon(Icons.lock_outline, color: _yellow, size: 32),
            const SizedBox(height: 8),
            Text(title, textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.bold)),
            const SizedBox(height: 6),
            Text(tr('Доступно после полной авторизации'), textAlign: TextAlign.center, style: const TextStyle(color: _yellow)),
            const SizedBox(height: 6),
            Text(tr('Нажмите, чтобы узнать подробнее'), textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ]),
        ),
      ),
    ),
  );
}
