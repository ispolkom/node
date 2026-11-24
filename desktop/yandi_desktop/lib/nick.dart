import 'package:flutter/material.dart';
import 'preferences.dart';
import 'transport/api.dart';

/// Ник — единственное имя человека: и в переписке, и в группах. Пока профиль не заполнен, в нём лежит короткий номер узла.
String? nickOf(Map? profile) {
  if (profile == null) return null;
  final name = '${profile['display_name'] ?? ''}'.trim();
  final short = '${profile['short_id'] ?? ''}'.trim();
  return name.isEmpty || name == short ? null : name;
}

/// true — профиль прочитан и ника в нём нет (ошибка чтения профиля — не повод требовать ник).
Future<bool> nickMissing(Api api) async {
  try {
    final r = await api.request('/api/profile/me');
    final p = r['profile'];
    return p is Map && nickOf(p) == null;
  } catch (_) {
    return false;
  }
}

Future<String?> loadNick(Api api) async {
  try {
    final r = await api.request('/api/profile/me');
    final p = r['profile'];
    return nickOf(p is Map ? p : null);
  } catch (_) {
    return null;
  }
}

/// Обязательный ник при создании профиля: окно не закрывается, пока ник не введён (аватар — по желанию, в профиле).
Future<String?> requireNick(BuildContext context, Api api) async {
  final ctrl = TextEditingController();
  String? error;
  String? saved;
  await showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (ctx) => PopScope(
      canPop: false,
      child: StatefulBuilder(
        builder: (ctx, setS) {
          Future<void> save() async {
            final nick = ctrl.text.trim();
            if (nick.isEmpty || nick.runes.length > 32) {
              setS(() => error = 'Введите ник (до 32 знаков): без него вас не увидят другие');
              return;
            }
            try {
              await api.request('/api/profile/me', method: 'PUT', body: {'display_name': nick});
              saved = nick;
              if (ctx.mounted) Navigator.pop(ctx);
            } catch (e) {
              setS(() => error = e is ApiFailure ? e.message : 'Не удалось сохранить ник');
            }
          }

          return AlertDialog(
            title: const UiText('Как вас называть?'),
            content: SizedBox(
              width: 360,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  TextField(
                    controller: ctrl,
                    autofocus: true,
                    maxLength: 32,
                    decoration: const InputDecoration(labelText: 'Ник (обязательно)'),
                    onSubmitted: (_) => save(),
                  ),
                  const UiText('Картинку профиля можно добавить позже (не обязательно).'),
                  if (error != null) UiText(error!, style: const TextStyle(color: Colors.red)),
                ],
              ),
            ),
            actions: [TextButton(onPressed: save, child: const UiText('Сохранить'))],
          );
        },
      ),
    ),
  );
  ctrl.dispose();
  return saved;
}
