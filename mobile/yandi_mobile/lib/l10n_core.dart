import 'l10n_en.dart';

/// Язык интерфейса: русский или английский. Здесь только чистый Dart (без Flutter): этим ядром пользуются и модели/службы, которые собираются
/// и проверяются отдельно от Flutter. Переключатель, запоминание и перерисовка — в `l10n.dart`.
enum AppLang { ru, en }

/// Текущий язык; меняет его `Lang` из `l10n.dart` (и тесты).
AppLang currentAppLang = AppLang.ru;

bool get isRuLang => currentAppLang == AppLang.ru;

/// Надпись на выбранном языке: ключ — русский текст; английский берётся из таблицы [enTable] (нет перевода — остаётся русский, это ловит тест).
String tr(String ru) => isRuLang ? ru : (enTable[ru] ?? ru);

/// Надпись с подстановками: в тексте места вида `{имя}` заменяются значениями.
String trf(String ru, Map<String, Object?> args) {
  var s = tr(ru);
  args.forEach((k, v) => s = s.replaceAll('{$k}', '$v'));
  return s;
}
