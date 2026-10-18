import 'package:flutter/material.dart';
import 'preferences.dart';
import 'theme.dart';
import 'transport/api.dart';

/// Вход в админ-панель: пароль администратора на 15 минут (сеанс держит узел; docs/ROLES.md). Возвращает true при успехе.
class AdminPasswordDialog extends StatefulWidget {
  final Api api;
  const AdminPasswordDialog({super.key, required this.api});
  @override
  State<AdminPasswordDialog> createState() => _AdminPasswordDialogState();
}

class _AdminPasswordDialogState extends State<AdminPasswordDialog> {
  final password = TextEditingController();
  String? error;
  bool busy = false;

  @override
  void dispose() {
    password.clear();
    password.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    if (busy || password.text.isEmpty) return;
    setState(() { busy = true; error = null; });
    try {
      await widget.api.request('/api/admin/session', body: {'password': password.text});
      password.clear();
      if (mounted) Navigator.pop(context, true);
    } on ApiFailure catch (e) {
      password.clear();
      if (!mounted) return;
      setState(() {
        busy = false;
        error = switch (e.status) {
          401 => 'Неверный пароль администратора',
          403 => 'Слишком много неверных паролей: войдите по сид-фразе (восстановление доступа)',
          429 => 'Слишком много попыток. Подождите перед повтором.',
          _ => e.message,
        };
      });
    } catch (_) {
      password.clear();
      if (mounted) setState(() { busy = false; error = 'Не удалось проверить пароль.'; });
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const UiText('Пароль администратора'),
    content: SizedBox(
      width: 380,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const UiText('Админ-панель управляет узлом: жалобы, сопряжение, настройки. Переписку она не открывает. Доступ на 15 минут.'),
          const SizedBox(height: 12),
          TextField(
            controller: password,
            autofocus: true,
            obscureText: true,
            enableSuggestions: false,
            autocorrect: false,
            enabled: !busy,
            decoration: InputDecoration(labelText: tr(context, 'Пароль администратора'), border: const OutlineInputBorder()),
            onSubmitted: (_) => submit(),
          ),
          if (error != null) Padding(padding: const EdgeInsets.only(top: 8), child: UiText(error!)),
        ],
      ),
    ),
    actions: [
      TextButton(onPressed: busy ? null : () => Navigator.pop(context, false), child: const UiText('Отмена')),
      SolidButton(busy ? 'Подождите…' : 'Войти', onPressed: busy ? null : submit),
    ],
  );
}

/// Извещения администратора в личном аккаунте: только тип, подпись и число. Действие — через админ-панель (по паролю).
class NoticesCard extends StatelessWidget {
  final List<Map<String, dynamic>> items;
  final VoidCallback onOpen;
  const NoticesCard({super.key, required this.items, required this.onOpen});
  @override
  Widget build(BuildContext context) => items.isEmpty
      ? const SizedBox.shrink()
      : Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: Panel(
            title: 'Извещения администратора',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final i in items)
                  Container(
                    margin: const EdgeInsets.only(bottom: 6),
                    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                    decoration: BoxDecoration(color: Colors.white, border: Border.all(color: Icq.border)),
                    child: Row(
                      children: [
                        if (i['new'] == true) const Padding(padding: EdgeInsets.only(right: 8), child: Icon(Icons.circle, size: 10, color: Color(0xffb3261e))),
                        Expanded(child: UiText('${i['label']}')),
                        Text('${i['count']}', style: const TextStyle(fontWeight: FontWeight.bold, fontStyle: FontStyle.italic)),
                      ],
                    ),
                  ),
                const SizedBox(height: 4),
                Align(alignment: Alignment.centerLeft, child: SolidButton('Открыть админ-панель', onPressed: onOpen)),
                const SizedBox(height: 4),
                const UiText('Подробности и действия — в админ-панели, по паролю администратора.', style: TextStyle(fontSize: 12, color: Icq.dark)),
              ],
            ),
          ),
        );
}

/// Отдельный пароль администратора: открывает только админ-панель, не переписку. Без него админ-панель открывается паролем пользователя.
class AdminPasswordPanel extends StatefulWidget {
  final Api api;
  const AdminPasswordPanel({super.key, required this.api});
  @override
  State<AdminPasswordPanel> createState() => _AdminPasswordPanelState();
}

class _AdminPasswordPanelState extends State<AdminPasswordPanel> {
  final current = TextEditingController(), next = TextEditingController(), repeat = TextEditingController();
  bool? separate;
  bool busy = false;
  String? error, done;

  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    for (final c in [current, next, repeat]) {
      c.clear();
      c.dispose();
    }
    super.dispose();
  }

  Future<void> load() async {
    try {
      final r = await widget.api.request('/api/admin/session');
      if (mounted) setState(() => separate = r['separate'] == true);
    } catch (_) {}
  }

  Future<void> save() async {
    if (busy) return;
    if (next.text != repeat.text) {
      setState(() { error = 'Пароли не совпадают'; done = null; });
      return;
    }
    setState(() { busy = true; error = null; done = null; });
    try {
      await widget.api.request('/api/admin/password', body: {'current': current.text, 'new': next.text, 'repeat': repeat.text});
      for (final c in [current, next, repeat]) {
        c.clear();
      }
      if (mounted) setState(() { busy = false; separate = true; done = 'Пароль администратора сохранён.'; });
    } on ApiFailure catch (e) {
      if (mounted) setState(() { busy = false; error = e.status == 429 ? 'Слишком много попыток. Подождите перед повтором.' : e.message; });
    } catch (_) {
      if (mounted) setState(() { busy = false; error = 'Не удалось сохранить пароль.'; });
    }
  }

  Widget field(TextEditingController c, String label) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 6),
    child: TextField(
      controller: c,
      obscureText: true,
      enableSuggestions: false,
      autocorrect: false,
      enabled: !busy,
      decoration: InputDecoration(labelText: tr(context, label), border: const OutlineInputBorder()),
    ),
  );

  @override
  Widget build(BuildContext context) => Panel(
    title: 'Пароль администратора',
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        UiText(separate == true
            ? 'Отдельный пароль администратора назначен: он открывает только админ-панель. Забыли его — войдите по сид-фразе, пароль вернётся к паролю пользователя.'
            : 'Сейчас админ-панель открывается паролем пользователя. Назначьте отдельный пароль: он открывает только админ-панель, не переписку.'),
        field(current, 'Текущий пароль администратора'),
        field(next, 'Новый пароль администратора'),
        field(repeat, 'Повтор нового пароля'),
        if (error != null) NoticePanel(error!, error: true),
        if (done != null) NoticePanel(done!),
        SolidButton(busy ? 'Подождите…' : 'Сохранить пароль администратора', onPressed: busy ? null : save),
      ],
    ),
  );
}
