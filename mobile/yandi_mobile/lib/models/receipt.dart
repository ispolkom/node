import 'dart:convert';

/// Служебные E2E-конверты между устройствами владельца для статусов доставки.
///
/// Всё идёт как обычное зашифрованное сообщение (узел видит только шифртекст и не отличает его от чата). Приложение по метке в начале
/// текста понимает, что это не чат: конверт сообщения (несёт id отправителя, чтобы получатель мог прислать квитанцию), квитанцию
/// «доставлено»/«прочитано» или пинг поддержки. Старые сборки этих меток не знают: пинг и квитанции идут «вживую»/в почте и у старой
/// сборки просто игнорируются, а конверт сообщения мы НЕ отправляем, пока собеседник не подтвердил поддержку (иначе показалась бы метка).
class MsgChannel {
  static const String msgMarker  = '\u0001yandi-msg:';   // {cmid, text} — обычное сообщение с id для квитанций
  static const String rcptMarker = '\u0001yandi-rcpt:';  // {kind: delivered|read, ids: [...]} — квитанция
  static const String capMarker  = '\u0001yandi-cap:';   // поддержка квитанций этим устройством (пинг)

  /// Завернуть текст с его локальным id (cmid), чтобы получатель мог сослаться на него в квитанции.
  static String wrapMessage(String cmid, String text) =>
      msgMarker + jsonEncode({'v': 1, 'cmid': cmid, 'text': text});

  static String capPing() => '${capMarker}1';

  static String receipt(String kind, List<String> ids) =>
      rcptMarker + jsonEncode({'kind': kind, 'ids': ids});

  static bool isService(String t) =>
      t.startsWith(msgMarker) || t.startsWith(rcptMarker) || t.startsWith(capMarker);

  static bool isCapPing(String t) => t.startsWith(capMarker);

  /// Разобрать конверт сообщения: (cmid, text). null — это не конверт.
  static (String, String)? parseMessage(String t) {
    if (!t.startsWith(msgMarker)) return null;
    try {
      final m = jsonDecode(t.substring(msgMarker.length)) as Map<String, dynamic>;
      final cmid = m['cmid'] as String?;
      final text = m['text'] as String?;
      if (cmid == null || text == null || cmid.isEmpty) return null;
      return (cmid, text);
    } catch (_) {
      return null;
    }
  }

  /// Разобрать квитанцию: (kind, ids). kind — "delivered" или "read".
  static (String, List<String>)? parseReceipt(String t) {
    if (!t.startsWith(rcptMarker)) return null;
    try {
      final m = jsonDecode(t.substring(rcptMarker.length)) as Map<String, dynamic>;
      final kind = m['kind'] as String?;
      final ids = (m['ids'] as List?)?.whereType<String>().toList();
      if (kind == null || ids == null || (kind != 'delivered' && kind != 'read')) return null;
      return (kind, ids);
    } catch (_) {
      return null;
    }
  }
}
