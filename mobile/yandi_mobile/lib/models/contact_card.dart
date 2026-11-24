import 'dart:convert';

/// Контакт, которым поделился собеседник в личном чате: `\u0001yandi-ct:{"s":16 hex,"i":64 hex?,"n":имя}`.
/// Сохраняется только по решению человека (окно с именем); в группы такие карточки не отправляются.
class ContactCardMsg {
  static const marker = '\u0001yandi-ct:';
  final String shortId, fullId, name;
  const ContactCardMsg(this.shortId, this.fullId, this.name);

  static String encode(String shortId, String fullId, String name) =>
      '$marker${jsonEncode({'s': shortId, if (fullId.length == 64) 'i': fullId, 'n': name})}';

  static ContactCardMsg? tryParse(String text) {
    if (!text.startsWith(marker)) return null;
    try {
      final j = jsonDecode(text.substring(marker.length));
      if (j is! Map) return null;
      final short = '${j['s'] ?? ''}'.toLowerCase();
      if (!RegExp(r'^[0-9a-f]{16}$').hasMatch(short)) return null;
      var full = '${j['i'] ?? ''}'.toLowerCase();
      if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(full) || !full.startsWith(short)) full = '';
      var n = '${j['n'] ?? ''}'.replaceAll(RegExp(r'[\x00-\x1f]'), '').trim();
      if (n.length > 64) n = n.substring(0, 64);
      return ContactCardMsg(short, full, n);
    } catch (_) {
      return null;
    }
  }
}
