import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../theme.dart';

/// Собственная клавиатура внутри приложения. Системная клавиатура (Gboard, клавиатура производителя) видит каждое нажатие, может учиться на
/// вводе и отправлять его в облако; поля с этой клавиатурой открываются с `TextInputType.none`, и система вообще не подключается к вводу.
/// Платой за это — нет автозамены, подсказок и голосового ввода. Раскладки: русская, английская, цифры и знаки, собственная панель эмодзи.
class SecureKeyboard extends StatefulWidget {
  final TextEditingController controller;
  /// Что делает «ввод»: по умолчанию вставляет перенос строки.
  final VoidCallback? onEnter;
  const SecureKeyboard({super.key, required this.controller, this.onEnter});

  /// Поле ввода для этой клавиатуры: системная не показывается и не учится (подсказки, автозамена и персонализация выключены).
  static const TextInputType inputType = TextInputType.none;

  @override
  State<SecureKeyboard> createState() => _SecureKeyboardState();
}

enum _Layer { letters, symbols, symbols2, emoji }

const _ru = [
  ['й', 'ц', 'у', 'к', 'е', 'н', 'г', 'ш', 'щ', 'з', 'х', 'ъ'],
  ['ф', 'ы', 'в', 'а', 'п', 'р', 'о', 'л', 'д', 'ж', 'э'],
  ['я', 'ч', 'с', 'м', 'и', 'т', 'ь', 'б', 'ю', 'ё'],
];
const _en = [
  ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'],
  ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'],
  ['z', 'x', 'c', 'v', 'b', 'n', 'm'],
];
const _sym1 = [
  ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'],
  ['-', '/', ':', ';', '(', ')', '₽', '&', '@', '"'],
  ['.', ',', '?', '!', "'", '№', '…'],
];
const _sym2 = [
  ['[', ']', '{', '}', '#', '%', '^', '*', '+', '='],
  ['_', '\\', '|', '~', '<', '>', '€', '£', '¥', '•'],
  ['.', ',', '?', '!', "'", '«', '»'],
];
const emojiPanel = [
  '😀', '😂', '🥹', '😊', '😍', '🥰', '😘', '😎', '🤔', '😅', '😭', '😡', '🥳', '😴', '🤯', '😱',
  '👍', '👎', '👏', '🙏', '💪', '👋', '🤝', '✌️', '👌', '🤞', '🫡', '🙌', '❤️', '💔', '🔥', '✨',
  '🎉', '🎁', '☀️', '🌙', '⭐', '🌈', '🍀', '🌸', '🍕', '☕', '🍺', '🎂', '🚗', '✈️', '🏠', '📞',
  '🐱', '🐶', '🦊', '🐼', '🦁', '🐸', '🐵', '🦉', '🐧', '🐢', '🦄', '🐙', '🔑', '🛡️', '💎', '✅',
];

class _SecureKeyboardState extends State<SecureKeyboard> {
  static String _lang = 'ru';          // раскладка помнится, пока приложение запущено
  _Layer _layer = _Layer.letters;
  int _shift = 0;                      // 0 — строчные, 1 — одна заглавная, 2 — Caps Lock
  Timer? _repeat;

  TextEditingController get c => widget.controller;

  @override
  void dispose() {
    _repeat?.cancel();
    super.dispose();
  }

  // ── редактирование текста в позиции курсора ──

  void _insert(String s) {
    HapticFeedback.selectionClick();
    final v = c.value;
    final sel = v.selection.isValid ? v.selection : TextSelection.collapsed(offset: v.text.length);
    final text = v.text.replaceRange(sel.start, sel.end, s);
    c.value = TextEditingValue(text: text, selection: TextSelection.collapsed(offset: sel.start + s.length));
    if (_shift == 1) setState(() => _shift = 0);
  }

  /// Удаляет выделенное или один символ ПЕРЕД курсором; символ — это графема целиком (эмодзи с модификаторами и семейные — одним нажатием).
  void _backspace() {
    final v = c.value;
    final sel = v.selection.isValid ? v.selection : TextSelection.collapsed(offset: v.text.length);
    if (!sel.isCollapsed) {
      c.value = TextEditingValue(text: v.text.replaceRange(sel.start, sel.end, ''), selection: TextSelection.collapsed(offset: sel.start));
      return;
    }
    if (sel.start == 0) return;
    final before = v.text.substring(0, sel.start);
    final kept = before.characters.skipLast(1).toString();
    c.value = TextEditingValue(text: kept + v.text.substring(sel.start), selection: TextSelection.collapsed(offset: kept.length));
  }

  void _startRepeat() {
    _backspace();
    HapticFeedback.selectionClick();
    _repeat?.cancel();
    _repeat = Timer.periodic(const Duration(milliseconds: 70), (_) => _backspace());
  }

  void _stopRepeat() {
    _repeat?.cancel();
    _repeat = null;
  }

  void _enter() {
    if (widget.onEnter != null) {
      widget.onEnter!();
    } else {
      _insert('\n');
    }
  }

  // ── вид ──

  String _letter(String l) => _shift == 0 ? l : l.toUpperCase();

  Widget _key(String label, VoidCallback onTap, {int flex = 1, Color? bg, Widget? child, VoidCallback? onLongPressStart, VoidCallback? onLongPressEnd, String? semantics}) {
    return Expanded(
      flex: flex,
      child: Padding(
        padding: const EdgeInsets.all(2.5),
        child: Semantics(
          label: semantics ?? label,
          button: true,
          child: GestureDetector(
            onLongPressStart: onLongPressStart == null ? null : (_) => onLongPressStart(),
            onLongPressEnd: onLongPressEnd == null ? null : (_) => onLongPressEnd(),
            onLongPressCancel: onLongPressEnd,
            child: Material(
              color: bg ?? AppTheme.surface,
              borderRadius: BorderRadius.circular(8),
              child: InkWell(
                borderRadius: BorderRadius.circular(8),
                onTap: onTap,
                child: SizedBox(
                  height: 46,
                  child: Center(child: child ?? Text(label, style: const TextStyle(color: AppTheme.text, fontSize: 18))),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _row(List<Widget> keys) => Row(children: keys);

  Widget _charKey(String l) => _key(_letter(l), () => _insert(_letter(l)));

  Widget _backspaceKey() => _key('⌫', _backspace,
      flex: 2, bg: AppTheme.divider, semantics: 'Удалить',
      child: const Icon(Icons.backspace_outlined, color: AppTheme.text, size: 20),
      onLongPressStart: _startRepeat, onLongPressEnd: _stopRepeat);

  Widget _bottomRow({required bool letters}) {
    return _row([
      _key(letters ? '123' : 'АБВ', () => setState(() => _layer = letters ? _Layer.symbols : _Layer.letters), flex: 2, bg: AppTheme.divider,
          child: Text(letters ? '123' : (_lang == 'ru' ? 'АБВ' : 'ABC'), style: const TextStyle(color: AppTheme.text, fontSize: 14)), semantics: letters ? 'Цифры и знаки' : 'Буквы'),
      _key('🌐', () => setState(() { _lang = _lang == 'ru' ? 'en' : 'ru'; _layer = _Layer.letters; }), flex: 2, bg: AppTheme.divider,
          child: Text(_lang == 'ru' ? 'RU' : 'EN', style: const TextStyle(color: AppTheme.accent, fontSize: 14, fontWeight: FontWeight.bold)), semantics: 'Сменить язык'),
      _key('😀', () => setState(() => _layer = _Layer.emoji), flex: 2, bg: AppTheme.divider, semantics: 'Эмодзи'),
      _key(' ', () => _insert(' '), flex: 6, child: const Text('пробел', style: TextStyle(color: AppTheme.textSecondary, fontSize: 13)), semantics: 'Пробел'),
      _key('⏎', _enter, flex: 2, bg: AppTheme.accent.withOpacity(0.25), child: const Icon(Icons.keyboard_return, color: AppTheme.text, size: 20), semantics: 'Ввод'),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final Widget body;
    switch (_layer) {
      case _Layer.letters:
        final rows = _lang == 'ru' ? _ru : _en;
        body = Column(mainAxisSize: MainAxisSize.min, children: [
          _row([for (final l in rows[0]) _charKey(l)]),
          _row([for (final l in rows[1]) _charKey(l)]),
          _row([
            _key('⇧', () => setState(() => _shift = _shift == 0 ? 1 : 0), flex: 2, bg: _shift == 0 ? AppTheme.divider : AppTheme.accent.withOpacity(0.5),
                child: Icon(_shift == 2 ? Icons.keyboard_capslock : Icons.keyboard_arrow_up, color: AppTheme.text), semantics: 'Заглавные',
                onLongPressStart: () => setState(() => _shift = 2)),
            for (final l in rows[2]) _charKey(l),
            _backspaceKey(),
          ]),
          _bottomRow(letters: true),
        ]);
      case _Layer.symbols:
      case _Layer.symbols2:
        final rows = _layer == _Layer.symbols ? _sym1 : _sym2;
        body = Column(mainAxisSize: MainAxisSize.min, children: [
          _row([for (final l in rows[0]) _key(l, () => _insert(l))]),
          _row([for (final l in rows[1]) _key(l, () => _insert(l))]),
          _row([
            _key(_layer == _Layer.symbols ? '#+=' : '123', () => setState(() => _layer = _layer == _Layer.symbols ? _Layer.symbols2 : _Layer.symbols), flex: 2, bg: AppTheme.divider,
                child: Text(_layer == _Layer.symbols ? '#+=' : '123', style: const TextStyle(color: AppTheme.text, fontSize: 14)), semantics: 'Другие знаки'),
            for (final l in rows[2]) _key(l, () => _insert(l)),
            _backspaceKey(),
          ]),
          _bottomRow(letters: false),
        ]);
      case _Layer.emoji:
        body = Column(mainAxisSize: MainAxisSize.min, children: [
          SizedBox(
            height: 46 * 3 + 15,
            child: GridView.count(
              crossAxisCount: 8,
              padding: const EdgeInsets.all(2),
              children: [for (final e in emojiPanel) InkWell(onTap: () => _insert(e), child: Center(child: Text(e, style: const TextStyle(fontSize: 24))))],
            ),
          ),
          _row([
            _key('АБВ', () => setState(() => _layer = _Layer.letters), flex: 2, bg: AppTheme.divider,
                child: const Text('АБВ', style: TextStyle(color: AppTheme.text, fontSize: 14)), semantics: 'Буквы'),
            _key(' ', () => _insert(' '), flex: 6, child: const Text('пробел', style: TextStyle(color: AppTheme.textSecondary, fontSize: 13)), semantics: 'Пробел'),
            _backspaceKey(),
          ]),
        ]);
    }
    return Material(
      color: AppTheme.bg,
      child: SafeArea(top: false, child: Padding(padding: const EdgeInsets.fromLTRB(2, 4, 2, 4), child: body)),
    );
  }
}
