import '../l10n_core.dart';

/// Надгробие: сообщение, удалённое вместе с пользователем (устройство убрано с узла). Узел и клиенты подменяют текст этой меткой.
/// Саму метку человеку не показываем никогда: везде, где выводится текст, идёт локализованная подпись.
class Tombstone {
  const Tombstone._();

  /// Точная строка-метка (часть протокола узла).
  static const marker = '[[yandi:deleted-with-user]]';

  static bool isMarker(String? text) => text == marker;

  /// Подпись надгробия на языке интерфейса.
  static String get label => tr('Сообщение удалено вместе с пользователем');

  /// Для превью и уведомлений: метка → подпись, иначе текст как есть.
  static String shown(String text) => text == marker ? label : text;
}
