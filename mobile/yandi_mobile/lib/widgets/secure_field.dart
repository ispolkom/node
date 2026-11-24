import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../theme.dart';
import 'secure_keyboard.dart';
import '../l10n.dart';

/// Поле ввода со своей клавиатурой приложения: системная не подключается к вводу вовсе (как в чате и группах).
/// Под полем — «Вставить» (из буфера обмена) и «Очистить»; клавиатура появляется под полем, пока оно в фокусе.
class SecureField extends StatefulWidget {
  final TextEditingController controller;
  final String hint;
  final int minLines;
  final int maxLines;
  final bool enabled;
  final double fontSize;
  final VoidCallback? onSubmitted;
  const SecureField({
    super.key,
    required this.controller,
    required this.hint,
    this.minLines = 1,
    this.maxLines = 1,
    this.enabled = true,
    this.fontSize = 14,
    this.onSubmitted,
  });

  @override
  State<SecureField> createState() => _SecureFieldState();
}

class _SecureFieldState extends State<SecureField> {
  final FocusNode _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.addListener(() { if (mounted) setState(() {}); });
  }

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  Future<void> _paste() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final t = data?.text;
    if (t == null || t.isEmpty) return;
    final c = widget.controller;
    final v = c.value;
    final sel = v.selection.isValid ? v.selection : TextSelection.collapsed(offset: v.text.length);
    final text = v.text.replaceRange(sel.start, sel.end, t);
    c.value = TextEditingValue(text: text, selection: TextSelection.collapsed(offset: sel.start + t.length));
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        TextField(
          controller: widget.controller,
          focusNode: _focus,
          enabled: widget.enabled,
          minLines: widget.minLines,
          maxLines: widget.maxLines,
          style: TextStyle(color: AppTheme.text, fontSize: widget.fontSize),
          // система к вводу не подключается: ни подсказок, ни обучения на нажатиях, ни облака
          keyboardType: SecureKeyboard.inputType,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
          decoration: InputDecoration(
            hintText: widget.hint,
            hintStyle: const TextStyle(color: AppTheme.textSecondary),
            filled: true,
            fillColor: AppTheme.surface,
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(10), borderSide: BorderSide.none),
          ),
        ),
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            TextButton.icon(
              onPressed: widget.enabled ? _paste : null,
              icon: const Icon(Icons.content_paste, size: 18, color: AppTheme.accent),
              label: Text(tr('Вставить'), style: TextStyle(color: AppTheme.accent)),
            ),
            TextButton.icon(
              onPressed: widget.enabled ? () => widget.controller.clear() : null,
              icon: const Icon(Icons.backspace_outlined, size: 18, color: AppTheme.textSecondary),
              label: Text(tr('Очистить'), style: TextStyle(color: AppTheme.textSecondary)),
            ),
          ],
        ),
        if (_focus.hasFocus) SecureKeyboard(controller: widget.controller, onEnter: widget.onSubmitted),
      ],
    );
  }
}
