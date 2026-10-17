import 'package:flutter/material.dart';
import 'preferences.dart';

/// The web UI's ICQ palette, shared by every desktop screen.
abstract final class Icq {
  static const background = Color(0xffe8f0e8),
      panel = Color(0xffd4e0d4),
      header = Color(0xff2a6b2a),
      headerLight = Color(0xff3a7b3a),
      border = Color(0xffa0c0a0),
      dark = Color(0xff608060),
      light = Color(0xfff0fff0),
      button = Color(0xffe0e0e0),
      text = Color(0xff1a3a1a),
      online = Color(0xff00aa00),
      danger = Color(0xffaa0000),
      sent = Color(0xffe0f0e0),
      active = Color(0xffc0e0c0),
      peer = Color(0xffff4001),
      mine = Color(0xff00d4aa);
  static const bevel = Border(
    top: BorderSide(color: light),
    left: BorderSide(color: light),
    right: BorderSide(color: dark),
    bottom: BorderSide(color: dark),
  );
  static const frame = Border(
    top: BorderSide(color: border),
    left: BorderSide(color: border),
    right: BorderSide(color: dark),
    bottom: BorderSide(color: dark),
  );

  static ThemeData get theme {
    final base = ThemeData(
      useMaterial3: false,
      fontFamily: 'Tahoma',
      fontFamilyFallback: const [
        'MS Sans Serif',
        'Segoe UI',
        'DejaVu Sans',
        'NotoColorEmoji',
      ],
      colorScheme: const ColorScheme.light(
        primary: header,
        secondary: online,
        surface: panel,
        error: danger,
        onSurface: text,
        onPrimary: Colors.white,
      ),
      scaffoldBackgroundColor: background,
      dividerColor: border,
      visualDensity: VisualDensity.compact,
      splashFactory: NoSplash.splashFactory,
    );
    final textTheme = base.textTheme
        .apply(bodyColor: text, displayColor: text)
        .copyWith(
          bodyMedium: const TextStyle(fontSize: 15, color: text),
          bodyLarge: const TextStyle(fontSize: 15, color: text),
          bodySmall: const TextStyle(fontSize: 12, color: dark),
          titleMedium: const TextStyle(fontSize: 16, color: text),
          labelLarge: const TextStyle(fontSize: 14, color: text),
        );
    // Команды — слова, а не кнопки: без фона и рамки, подчёркнутые; главное действие — жирным.
    ButtonStyle wordStyle({bool primary = false}) => ButtonStyle(
      backgroundColor: const WidgetStatePropertyAll(Colors.transparent),
      overlayColor: WidgetStatePropertyAll(header.withValues(alpha: 0.10)),
      foregroundColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled) ? dark : header,
      ),
      shape: const WidgetStatePropertyAll(RoundedRectangleBorder()),
      elevation: const WidgetStatePropertyAll(0),
      shadowColor: const WidgetStatePropertyAll(Colors.transparent),
      padding: const WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      ),
      minimumSize: const WidgetStatePropertyAll(Size(0, 28)),
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      textStyle: WidgetStatePropertyAll(
        TextStyle(
          fontSize: 14,
          fontFamily: 'Tahoma',
          fontFamilyFallback: const ['NotoColorEmoji'],
          fontWeight: primary ? FontWeight.bold : FontWeight.normal,
          decoration: TextDecoration.underline,
        ),
      ),
    );
    const outline = OutlineInputBorder(
      borderRadius: BorderRadius.all(Radius.circular(2)),
      borderSide: BorderSide(color: border),
    );
    return base.copyWith(
      textTheme: textTheme.apply(
        fontFamily: 'Tahoma',
        fontFamilyFallback: const ['NotoColorEmoji'],
      ),
      filledButtonTheme: FilledButtonThemeData(style: wordStyle(primary: true)),
      elevatedButtonTheme: ElevatedButtonThemeData(
        style: wordStyle(primary: true),
      ),
      textButtonTheme: TextButtonThemeData(style: wordStyle()),
      outlinedButtonTheme: OutlinedButtonThemeData(style: wordStyle()),
      iconButtonTheme: IconButtonThemeData(
        style: ButtonStyle(
          backgroundColor: const WidgetStatePropertyAll(Colors.transparent),
          shape: const WidgetStatePropertyAll(RoundedRectangleBorder()),
          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        ),
      ),
      cardTheme: const CardThemeData(
        color: panel,
        elevation: 0,
        margin: EdgeInsets.symmetric(vertical: 4),
        shape: BevelBorder(),
      ),
      dialogTheme: const DialogThemeData(
        backgroundColor: panel,
        elevation: 0,
        shape: BevelBorder(),
        titleTextStyle: TextStyle(
          fontSize: 16,
          fontWeight: FontWeight.bold,
          color: header,
        ),
      ),
      inputDecorationTheme: const InputDecorationTheme(
        filled: true,
        fillColor: Colors.white,
        isDense: true,
        contentPadding: EdgeInsets.symmetric(horizontal: 10, vertical: 10),
        border: outline,
        enabledBorder: outline,
        disabledBorder: outline,
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.all(Radius.circular(2)),
          borderSide: BorderSide(color: header),
        ),
        labelStyle: TextStyle(fontSize: 12, color: dark),
        helperStyle: TextStyle(fontSize: 11, color: dark),
      ),
      switchTheme: SwitchThemeData(
        thumbColor: WidgetStateProperty.resolveWith(
          (s) => s.contains(WidgetState.selected) ? header : dark,
        ),
      ),
      listTileTheme: const ListTileThemeData(
        dense: true,
        contentPadding: EdgeInsets.symmetric(horizontal: 8),
      ),
      progressIndicatorTheme: const ProgressIndicatorThemeData(color: header),
      scrollbarTheme: ScrollbarThemeData(
        radius: const Radius.circular(2),
        thickness: const WidgetStatePropertyAll(12),
        thumbColor: const WidgetStatePropertyAll(dark),
      ),
    );
  }
}

class BevelBorder extends OutlinedBorder {
  const BevelBorder();
  @override
  EdgeInsetsGeometry get dimensions => const EdgeInsets.all(1);
  @override
  BevelBorder copyWith({BorderSide? side}) => const BevelBorder();
  @override
  ShapeBorder scale(double t) => this;
  @override
  Path getInnerPath(Rect rect, {TextDirection? textDirection}) =>
      Path()..addRect(rect.deflate(1));
  @override
  Path getOuterPath(Rect rect, {TextDirection? textDirection}) =>
      Path()..addRect(rect);
  @override
  void paint(Canvas canvas, Rect rect, {TextDirection? textDirection}) {
    Icq.bevel.paint(canvas, rect);
  }
}

class BevelButton extends TextButton {
  BevelButton({
    super.key,
    required String label,
    required super.onPressed,
    bool primary = false,
    bool danger = false,
  }) : super(
         style: danger
             ? TextButton.styleFrom(foregroundColor: Icq.danger)
             : (primary
                   ? TextButton.styleFrom(
                       textStyle: const TextStyle(
                         fontWeight: FontWeight.bold,
                         decoration: TextDecoration.underline,
                       ),
                     )
                   : null),
         child: UiText(label),
       );
}

/// Кнопка в цвете шапки приложения: основная — тёмно-зелёная, светлая — светлее, опасная — красная. Без подчёркнутых «слов-ссылок».
enum SolidTone { primary, light, danger }

class SolidButton extends StatelessWidget {
  final String label;
  final VoidCallback? onPressed;
  final SolidTone tone;
  const SolidButton(this.label, {super.key, required this.onPressed, this.tone = SolidTone.primary});
  @override
  Widget build(BuildContext context) => ElevatedButton(
    onPressed: onPressed,
    style: ElevatedButton.styleFrom(
      backgroundColor: switch (tone) {
        SolidTone.primary => Icq.header,
        SolidTone.light => Icq.headerLight,
        SolidTone.danger => const Color(0xffb3261e),
      },
      foregroundColor: Colors.white,
      disabledBackgroundColor: Icq.border,
      disabledForegroundColor: Colors.white70,
      elevation: 0,
      minimumSize: const Size(100, 40),
      padding: const EdgeInsets.symmetric(horizontal: 14),
      shape: const RoundedRectangleBorder(),
      side: const BorderSide(color: Icq.light),
      textStyle: const TextStyle(
        fontSize: 14,
        fontFamily: 'Tahoma',
        fontFamilyFallback: ['NotoColorEmoji'],
        decoration: TextDecoration.none,
      ),
    ),
    child: UiText(label, style: const TextStyle(color: Colors.white)),
  );
}

class PageHeader extends StatelessWidget {
  final String title;
  final Widget? trailing;
  const PageHeader(this.title, {super.key, this.trailing});
  @override
  Widget build(BuildContext context) => Container(
    width: double.infinity,
    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
    decoration: const BoxDecoration(
      color: Icq.header,
      border: Border(bottom: BorderSide(color: Icq.light)),
    ),
    child: Row(
      children: [
        Expanded(
          child: UiText(
            title,
            style: const TextStyle(
              color: Colors.white,
              fontSize: 16,
              fontWeight: FontWeight.bold,
            ),
          ),
        ),
        if (trailing != null) trailing!,
      ],
    ),
  );
}

class Panel extends StatelessWidget {
  final String? title;
  final Widget child;
  final Widget? trailing;
  final Color color;
  final EdgeInsets padding;
  const Panel({
    super.key,
    this.title,
    required this.child,
    this.trailing,
    this.color = Icq.panel,
    this.padding = const EdgeInsets.all(16),
  });
  @override
  Widget build(BuildContext context) => Container(
    width: double.infinity,
    decoration: BoxDecoration(color: color, border: Icq.frame),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (title != null) PageHeader(title!, trailing: trailing),
        Padding(padding: padding, child: child),
      ],
    ),
  );
}

class StatusRow extends StatelessWidget {
  final String label;
  final dynamic value;
  const StatusRow(this.label, this.value, {super.key});
  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(vertical: 8),
    decoration: const BoxDecoration(
      border: Border(bottom: BorderSide(color: Icq.border)),
    ),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: UiText(
            label,
            style: const TextStyle(color: Icq.dark, fontSize: 12),
          ),
        ),
        const SizedBox(width: 12),
        Expanded(child: SelectableText('$value', textAlign: TextAlign.right)),
      ],
    ),
  );
}

class NoticePanel extends StatelessWidget {
  final String text;
  final bool error;
  const NoticePanel(this.text, {super.key, this.error = false});
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Panel(
      title: error ? 'Ошибка' : 'Сообщение',
      padding: const EdgeInsets.all(12),
      child: UiText(
        text,
        style: TextStyle(color: error ? Icq.danger : Icq.text),
      ),
    ),
  );
}

class TabStrip extends StatelessWidget {
  final List<String> labels;
  final String selected;
  final ValueChanged<String>? onSelected;

  /// Header variant: light active tab on green (the web nav); otherwise the
  /// page variant: green active button among grey ones (the web `.btn-primary`).
  final bool header;
  const TabStrip({
    super.key,
    required this.labels,
    required this.selected,
    required this.onSelected,
    this.header = false,
  });
  @override
  Widget build(BuildContext context) => Wrap(
    spacing: header ? 4 : 6,
    runSpacing: 4,
    children: [
      for (final label in labels)
        OutlinedButton(
          style: OutlinedButton.styleFrom(
            backgroundColor: header
                ? (selected == label ? Icq.light : Icq.headerLight)
                : (selected == label ? Icq.header : Icq.button),
            foregroundColor: header
                ? (selected == label ? Icq.header : Colors.white)
                : (selected == label ? Colors.white : Icq.text),
            padding: EdgeInsets.symmetric(
              horizontal: header ? 10 : 14,
              vertical: header ? 6 : 8,
            ),
            textStyle: TextStyle(
              fontSize: header ? 13 : 14,
              fontFamily: 'Tahoma',
            ),
            minimumSize: const Size(0, 30),
          ),
          onPressed: onSelected == null ? null : () => onSelected!(label),
          child: UiText(label),
        ),
    ],
  );
}

/// Turns the existing feature sections into the web UI's vertical panel stack.
/// Keeps all controls mounted and preserves their original ordering.
class PanelStack extends StatelessWidget {
  final List<Widget> children;
  final String? title;
  const PanelStack({super.key, required this.children, this.title});
  @override
  Widget build(BuildContext context) {
    final sections = <Widget>[];
    var heading = title;
    var body = <Widget>[];
    void flush() {
      if (body.isEmpty && heading == null) return;
      sections.add(
        Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: Panel(
            title: heading,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final child in body)
                  if (child is TextField || child is DropdownButton)
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: 6),
                      child: child,
                    )
                  else if (child is ButtonStyleButton)
                    Align(alignment: Alignment.centerLeft, child: child)
                  else
                    child,
              ],
            ),
          ),
        ),
      );
      body = [];
    }

    for (final child in children) {
      if (child is UiText && (child.style?.fontSize ?? 0) >= 18) {
        flush();
        heading = child.text;
      } else {
        body.add(child);
      }
    }
    flush();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: sections,
    );
  }
}

class HeaderLayout extends StatelessWidget {
  final List<Widget> children;
  const HeaderLayout({super.key, required this.children});
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, size) {
      final tabs = (children[2] as Expanded).child;
      if (size.maxWidth < 1000) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                children[0],
                const SizedBox(width: 12),
                Expanded(
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: children[4],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            tabs,
          ],
        );
      }
      return Row(
        children: [
          children[0],
          children[1],
          Expanded(child: tabs),
          children[3],
          children[4],
        ],
      );
    },
  );
}
