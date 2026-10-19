import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'l10n_en.dart';

/// Язык интерфейса: русский или английский. По умолчанию — язык системы (русский, если он русский, иначе английский); выбор запоминается.
enum AppLang { ru, en }

class Lang {
  static final ValueNotifier<AppLang> current = ValueNotifier<AppLang>(_system());
  static bool get isRu => current.value == AppLang.ru;
  static const _key = 'ui_lang';

  static AppLang _system() => ui.PlatformDispatcher.instance.locale.languageCode.toLowerCase() == 'ru' ? AppLang.ru : AppLang.en;

  /// Прочитать сохранённый выбор (до запуска интерфейса). Ошибка чтения не мешает: остаётся язык системы.
  static Future<void> load() async {
    try {
      final v = (await SharedPreferences.getInstance()).getString(_key);
      if (v == 'ru') current.value = AppLang.ru;
      if (v == 'en') current.value = AppLang.en;
    } catch (_) {}
  }

  /// Сменить язык: запоминается, весь интерфейс перерисовывается.
  static Future<void> set(AppLang l) async {
    if (current.value == l) return;
    current.value = l;
    _rebuildAll();
    try {
      await (await SharedPreferences.getInstance()).setString(_key, l == AppLang.ru ? 'ru' : 'en');
    } catch (_) {}
  }

  /// Для тестов: задать язык без записи и перерисовки.
  static void forTests(AppLang l) => current.value = l;

  /// Надписи вычисляются при построении экрана, а часть экранов неизменяемые (`const`): после смены языка перестраиваем всё дерево.
  static void _rebuildAll() {
    void visit(Element e) {
      e.markNeedsBuild();
      e.visitChildren(visit);
    }

    WidgetsBinding.instance.rootElement?.visitChildren(visit);
  }
}

/// Надпись на выбранном языке: ключ — русский текст; английский берётся из таблицы [enTable] (нет перевода — остаётся русский, это ловит тест).
String tr(String ru) => Lang.isRu ? ru : (enTable[ru] ?? ru);

/// Надпись с подстановками: в тексте места вида `{имя}` заменяются значениями.
String trf(String ru, Map<String, Object?> args) {
  var s = tr(ru);
  args.forEach((k, v) => s = s.replaceAll('{$k}', '$v'));
  return s;
}

/// Переключатель «RU | EN»: для экранов, которые человек видит до входа.
class LangToggle extends StatelessWidget {
  const LangToggle({super.key});
  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<AppLang>(
      valueListenable: Lang.current,
      builder: (_, cur, __) {
        Widget b(AppLang l, String label) => InkWell(
              key: ValueKey('lang-${l.name}'),
              onTap: () => Lang.set(l),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                child: Text(label, style: TextStyle(fontWeight: cur == l ? FontWeight.bold : FontWeight.normal, decoration: cur == l ? TextDecoration.underline : TextDecoration.none, color: Theme.of(context).colorScheme.onSurface)),
              ),
            );
        return Row(mainAxisSize: MainAxisSize.min, children: [b(AppLang.ru, 'RU'), const Text('|'), b(AppLang.en, 'EN')]);
      },
    );
  }
}
