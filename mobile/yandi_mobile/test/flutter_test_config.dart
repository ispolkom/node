import 'dart:async';
import 'package:yandi_mobile/l10n.dart';

/// Тесты написаны под русский интерфейс: язык по умолчанию в тестах — русский (английский проверяется отдельными тестами).
Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  Lang.forTests(AppLang.ru);
  await testMain();
}
