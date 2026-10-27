/// Версия этой сборки приложения в виде `A.B.C-bN` (pubspec: `A.B.C+N`). Узел сравнивает её с подписанным манифестом выпуска.
/// Тест `test/app_version_test.dart` следит, чтобы она не разошлась с pubspec.yaml.
const kAppVersion = '1.0.0-b7';

/// Старше ли версия [a], чем [b] (числа по порядку: `1.0.0-b4` → 1,0,0,4). Для отзыва сборок (`min_app` узла).
bool versionOlder(String a, String b) {
  List<int> parts(String v) => [for (final m in RegExp(r'\d+').allMatches(v)) int.parse(m.group(0)!)];
  final x = parts(a), y = parts(b);
  for (var i = 0; i < x.length || i < y.length; i++) {
    final p = i < x.length ? x[i] : 0, q = i < y.length ? y[i] : 0;
    if (p != q) return p < q;
  }
  return false;
}
