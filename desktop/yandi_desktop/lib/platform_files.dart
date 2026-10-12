import 'package:flutter/services.dart';

/// GTK chooser; no dependency downloads and no shell-managed staging files.
class PlatformFiles {
  static const channel = MethodChannel('yandi/local');
  static Future<String?> choose({bool save = false, String name = ''}) =>
      channel.invokeMethod<String>('choose', {'save': save, 'name': name});
  static Future<void> cancel() => channel.invokeMethod<void>('cancelChooser');
  static Future<void> open(String path) =>
      channel.invokeMethod<void>('open', path);
}
