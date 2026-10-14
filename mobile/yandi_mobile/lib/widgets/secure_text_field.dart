import 'package:flutter/material.dart';
import 'secure_keyboard.dart';

/// Замена `TextField`: система к вводу не подключается (ни подсказок, ни обучения на нажатиях, ни облака), печатать
/// можно только нашей клавиатурой, которая появляется под полем, пока оно в фокусе.
class SecureTextField extends StatefulWidget {
  final TextEditingController controller;
  final InputDecoration decoration;
  final TextStyle? style;
  final int? maxLength;
  final int? maxLines;
  final bool autofocus;
  final VoidCallback? onEnter;
  const SecureTextField({
    super.key,
    required this.controller,
    this.decoration = const InputDecoration(),
    this.style,
    this.maxLength,
    this.maxLines = 1,
    this.autofocus = false,
    this.onEnter,
  });

  @override
  State<SecureTextField> createState() => _SecureTextFieldState();
}

class _SecureTextFieldState extends State<SecureTextField> {
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

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextField(
          controller: widget.controller,
          focusNode: _focus,
          autofocus: widget.autofocus,
          maxLength: widget.maxLength,
          maxLines: widget.maxLines,
          style: widget.style,
          decoration: widget.decoration,
          keyboardType: SecureKeyboard.inputType,
          enableSuggestions: false,
          autocorrect: false,
          enableIMEPersonalizedLearning: false,
        ),
        if (_focus.hasFocus) SecureKeyboard(controller: widget.controller, onEnter: widget.onEnter),
      ],
    );
  }
}
