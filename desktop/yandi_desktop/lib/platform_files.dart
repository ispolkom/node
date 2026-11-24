import 'package:flutter/services.dart';

/// GTK chooser; no dependency downloads and no shell-managed staging files.
class PlatformFiles {
  static const channel = MethodChannel('yandi/local');
  /// Пока открыт системный диалог, окно оболочки теряет фокус — это не уход
  /// пользователя: содержимое не стираем, иначе выбор файла теряется.
  static int _open = 0;
  static bool get choosing => _open > 0;

  /// Вызывается после закрытия диалога: оболочка заново сверяет фокус окна.
  static void Function()? onClosed;

  static Future<String?> choose({bool save = false, String name = ''}) async {
    _open++;
    try {
      return await channel.invokeMethod<String>('choose', {
        'save': save,
        'name': name,
      });
    } finally {
      _open--;
      onClosed?.call();
    }
  }
  static Future<void> cancel() => channel.invokeMethod<void>('cancelChooser');
  static Future<void> open(String path) =>
      channel.invokeMethod<void>('open', path);
}
