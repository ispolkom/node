import 'package:flutter/material.dart';

class AppTheme {
  // Тёмная тема в стиле веб-интерфейса YANDI
  static const Color bg          = Color(0xFF0D1117);
  static const Color surface     = Color(0xFF161B22);
  static const Color accent      = Color(0xFF00D4AA);
  static const Color text        = Color(0xFFE6EDF3);
  static const Color textSecondary = Color(0xFF7D8590);
  static const Color divider     = Color(0xFF21262D);

  static ThemeData get dark => ThemeData(
    brightness: Brightness.dark,
    scaffoldBackgroundColor: bg,
    colorScheme: const ColorScheme.dark(
      primary:   accent,
      surface:   surface,
      onSurface: text,
    ),
    appBarTheme: const AppBarTheme(
      backgroundColor: surface,
      foregroundColor: text,
      elevation: 0,
    ),
    dividerColor: divider,
    textTheme: const TextTheme(
      bodyMedium: TextStyle(color: text),
      bodySmall:  TextStyle(color: textSecondary),
    ),
  );
}
