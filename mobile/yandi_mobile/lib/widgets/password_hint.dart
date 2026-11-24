import 'package:flutter/material.dart';
import '../crypto/password_strength.dart';

/// Строка под полем пароля: цвет и текст меняются, пока человек печатает (предупреждение, не запрет).
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
        PasswordLevel.weak => Colors.redAccent,
        PasswordLevel.medium => Colors.amber,
        _ => Colors.lightGreen,
      };
      return Padding(
        padding: const EdgeInsets.only(bottom: 6),
        child: Text(PasswordStrength.text(l), style: TextStyle(fontSize: 13, color: color)),
      );
    },
  );
}
