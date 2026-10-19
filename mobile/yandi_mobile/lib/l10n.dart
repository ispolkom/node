import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'l10n_core.dart';
export 'l10n_core.dart' show AppLang, tr, trf;

/// Язык интерфейса: русский или английский. По умолчанию — язык системы (русский, если он русский, иначе английский); выбор запоминается.
/// Чистое ядро (`tr`, `trf`, `AppLang`) лежит в `l10n_core.dart`.

class Lang {
  static final ValueNotifier<AppLang> current = ValueNotifier<AppLang>(_init());
  static bool get isRu => currentAppLang == AppLang.ru;
  static const _key = 'ui_lang';

  static AppLang _init() {
    final l = _system();
    currentAppLang = l;
    return l;
  }

  static AppLang _system() => ui.PlatformDispatcher.instance.locale.languageCode.toLowerCase() == 'ru' ? AppLang.ru : AppLang.en;

  /// Прочитать сохранённый выбор (до запуска интерфейса). Ошибка чтения не мешает: остаётся язык системы.
  static Future<void> load() async {
    try {
      final v = (await SharedPreferences.getInstance()).getString(_key);
      if (v == 'ru') _apply(AppLang.ru);
      if (v == 'en') _apply(AppLang.en);
    } catch (_) {}
  }

  /// Сменить язык: запоминается, весь интерфейс перерисовывается.
  static Future<void> set(AppLang l) async {
    if (current.value == l) return;
    _apply(l);
    _rebuildAll();
    try {
      await (await SharedPreferences.getInstance()).setString(_key, l == AppLang.ru ? 'ru' : 'en');
    } catch (_) {}
  }

  /// Для тестов: задать язык без записи и перерисовки.
  static void forTests(AppLang l) => _apply(l);

  static void _apply(AppLang l) {
    current.value = l; // сначала: первое обращение к current выполняет _init(), он не должен затереть заданный язык
    currentAppLang = l;
  }

  /// Надписи вычисляются при построении экрана, а часть экранов неизменяемые (`const`): после смены языка перестраиваем всё дерево.
  static void _rebuildAll() {
    void visit(Element e) {
      e.markNeedsBuild();
      e.visitChildren(visit);
    }

    WidgetsBinding.instance.rootElement?.visitChildren(visit);
  }
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
