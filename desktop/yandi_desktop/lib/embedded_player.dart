import 'dart:async';
import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';
import 'package:media_kit_video/media_kit_video.dart';
import 'theme.dart';

/// Встроенный в окно оболочки проигрыватель (libmpv через media_kit): видео и звук показываются в диалоге поверх чата, без отдельного
/// окна. Источник — тот же адрес петли 127.0.0.1, что и у внешнего проигрывателя: расшифрованные данные живут только в памяти оболочки.
/// Если библиотеки нет (libmpv не установлен), `enabled` остаётся false и оболочка работает через внешний проигрыватель, как раньше.
/// Пока идёт воспроизведение (окно проигрывателя открыто или запущен внешний проигрыватель), потеря фокуса главным окном не стирает
/// чат: иначе проигрыватель, забрав фокус, сразу закрывался бы вместе с чатом. Счётчик, а не флаг: проигрывателей может быть несколько.
abstract final class PlaybackGuard {
  static int _active = 0;
  static bool get active => _active > 0;
  static void enter() => _active++;
  static void leave() {
    if (_active > 0) _active--;
  }

  static void reset() => _active = 0;
}

class EmbeddedPlayer {
  /// Включается в `main` после успешной инициализации; тесты оставляют false (внешний путь).
  static bool enabled = false;

  /// Проверка при запуске: true — библиотеки на месте.
  static bool init() {
    try {
      MediaKit.ensureInitialized();
      enabled = true;
    } catch (_) {
      enabled = false;
    }
    return enabled;
  }

  /// Сигнал «закрыть все окна проигрывателя» (запирание оболочки, закрытие карточки): стирание памяти не должно ждать пользователя.
  static final ValueNotifier<int> closeAll = ValueNotifier<int>(0);

  /// Подменяется в тестах. Завершается, когда диалог закрыт.
  static Future<void> Function(BuildContext context, Uri url, String title, bool video) show = _show;

  static Future<void> _show(BuildContext context, Uri url, String title, bool video) => showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _PlayerDialog(url: url, title: title, video: video),
  );
}

class _PlayerDialog extends StatefulWidget {
  final Uri url;
  final String title;
  final bool video;
  const _PlayerDialog({required this.url, required this.title, required this.video});
  @override
  State<_PlayerDialog> createState() => _PlayerDialogState();
}

class _PlayerDialogState extends State<_PlayerDialog> {
  late final Player player = Player();
  late final VideoController controller = VideoController(player);
  String? error;
  StreamSubscription<String>? errors;

  @override
  void initState() {
    super.initState();
    EmbeddedPlayer.closeAll.addListener(_closeRequested);
    errors = player.stream.error.listen((e) {
      if (mounted) setState(() => error = 'Не удалось воспроизвести: файл повреждён или формат не поддерживается');
    });
    _open();
  }

  void _closeRequested() {
    if (mounted) Navigator.of(context).pop();
  }

  Future<void> _open() async {
    try {
      final native = player.platform;
      if (native is NativePlayer) {
        // адрес петли идёт напрямую, даже если в окружении пользователя стоит http_proxy
        await native.setProperty('stream-lavf-o', 'http_proxy=');
        await native.setProperty('network-timeout', '15');
      }
      await player.open(Media(widget.url.toString()), play: true);
    } catch (_) {
      if (mounted) setState(() => error = 'Не удалось открыть проигрыватель');
    }
  }

  @override
  void dispose() {
    EmbeddedPlayer.closeAll.removeListener(_closeRequested);
    errors?.cancel();
    player.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: Icq.panel,
      insetPadding: const EdgeInsets.all(24),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 1100, maxHeight: 720),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              color: Icq.header,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              child: Row(
                children: [
                  Expanded(child: Text(widget.title, overflow: TextOverflow.ellipsis, style: const TextStyle(color: Colors.white, fontWeight: FontWeight.bold))),
                  TextButton(onPressed: () => Navigator.of(context).pop(), child: const Text('Закрыть', style: TextStyle(color: Colors.white))),
                ],
              ),
            ),
            Flexible(
              child: AspectRatio(
                aspectRatio: widget.video ? 16 / 9 : 16 / 4,
                child: error != null
                    ? Center(child: Text(error!, style: const TextStyle(color: Color(0xFFC62828))))
                    : Video(controller: controller, controls: MaterialDesktopVideoControls),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
