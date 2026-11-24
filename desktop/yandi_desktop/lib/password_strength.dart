import 'dart:math' as math;
import 'package:flutter/material.dart';
import 'preferences.dart';

/// Оценка стойкости пароля для предупреждения (не запрет): пароль защищает данные от тех, кто скопировал файлы и перебирает офлайн,
/// а лимиты 5/20 попыток работают только внутри приложения.
enum PasswordLevel { empty, weak, medium, good }

class PasswordStrength {
  static const _common = {
    '123456', '1234567', '12345678', '123456789', '1234567890', '111111', '000000', 'password', 'qwerty', 'qwertyuiop', 'abc123',
    'iloveyou', 'admin', 'letmein', 'welcome', 'йцукен', 'пароль', '1q2w3e4r', '123123', 'qazwsx', 'password1', 'yandi',
  };

  /// Грубая оценка стойкости в битах (по алфавиту и длине, со штрафами за повторы, ряды и словарные пароли).
  static double bits(String p) {
    if (p.isEmpty) return 0;
    final lower = p.toLowerCase();
    if (_common.contains(lower)) return 8;
    // повтор одного и того же куска («passwordpassword», «abcabcabc») даёт стойкость куска плюс пару бит на число повторов
    final unit = _repeatedUnit(p);
    if (unit != null) return bits(unit) + 2;
    // слова через пробел: повторяющиеся слова не независимы, а словарное слово даёт почти ничего
    final tokens = lower.trim().split(RegExp(r'\s+')).where((w) => w.isNotEmpty).toList();
    if (tokens.length >= 2) {
      final distinct = tokens.toSet();
      if (distinct.length < tokens.length) return bits(distinct.join(' ')) + 2;
      final unusual = distinct.where((w) => !_common.contains(w)).toList();
      if (unusual.length < distinct.length && unusual.length < 3) return unusual.length * 13.0 + 8;
    }
    var pool = 0;
    if (RegExp(r'[a-z]').hasMatch(p)) pool += 26;
    if (RegExp(r'[A-Z]').hasMatch(p)) pool += 26;
    if (RegExp(r'[0-9]').hasMatch(p)) pool += 10;
    if (RegExp(r'[а-яё]').hasMatch(lower)) pool += 33;
    if (RegExp(r'[^A-Za-z0-9а-яёА-ЯЁ ]').hasMatch(p)) pool += 32;
    if (p.contains(' ')) pool += 1;
    if (pool == 0) pool = 10;
    final runes = p.runes.toList();
    // повторы и ряды (aaaaaa, 123456, abcdef) почти не добавляют стойкости: считаем только «разные по сути» знаки
    var effective = 0.0;
    for (var i = 0; i < runes.length; i++) {
      if (i >= 1 && runes[i] == runes[i - 1]) continue;
      if (i >= 2 && runes[i] - runes[i - 1] == runes[i - 1] - runes[i - 2] && (runes[i] - runes[i - 1]).abs() == 1) continue;
      effective += 1;
    }
    var b = effective * (math.log(pool) / math.ln2);
    // фраза из нескольких слов подбирается как набор слов, а не по знакам: засчитываем не больше ~13 бит на слово
    final words = p.trim().split(RegExp(r'\s+')).where((w) => w.length >= 3).length;
    if (words >= 3) b = math.min(b, words * 13.0 + 6);
    return b;
  }

  /// Если строка — повторение более короткого куска целое число раз, возвращает кусок.
  static String? _repeatedUnit(String p) {
    final n = p.length;
    for (var u = 1; u <= n ~/ 2; u++) {
      if (n % u != 0) continue;
      final unit = p.substring(0, u);
      if (unit * (n ~/ u) == p) return unit;
    }
    return null;
  }

  static PasswordLevel level(String p) {
    if (p.isEmpty) return PasswordLevel.empty;
    final b = bits(p);
    if (p.runes.length < 8 || b < 36) return PasswordLevel.weak;
    if (b < 52) return PasswordLevel.medium;
    return PasswordLevel.good;
  }

  static String text(PasswordLevel l) => switch (l) {
    PasswordLevel.empty => '',
    PasswordLevel.weak => 'Слабый пароль: его подберут за часы, если файлы скопируют. Возьмите фразу из нескольких слов.',
    PasswordLevel.medium => 'Средний пароль. Надёжнее фраза из нескольких слов или длиннее.',
    PasswordLevel.good => 'Хороший пароль.',
  };
}

/// Строка под полем пароля: цвет и текст меняются, пока человек печатает.
class PasswordHint extends StatelessWidget {
  final TextEditingController controller;
  const PasswordHint(this.controller, {super.key});
  @override
  Widget build(BuildContext context) => ValueListenableBuilder<TextEditingValue>(
    valueListenable: controller,
    builder: (context, v, _) {
      final l = PasswordStrength.level(v.text);
      if (l == PasswordLevel.empty) return const SizedBox.shrink();
      final color = switch (l) {
        PasswordLevel.weak => const Color(0xffb3261e),
        PasswordLevel.medium => const Color(0xff8a6d00),
        _ => const Color(0xff2a6b2a),
      };
      return Padding(
        padding: const EdgeInsets.only(bottom: 6),
        child: UiText(PasswordStrength.text(l), style: TextStyle(fontSize: 13, color: color)),
      );
    },
  );
}
