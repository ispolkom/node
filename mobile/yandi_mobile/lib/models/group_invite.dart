import 'dart:convert';

/// Приглашение в группу: номер группы + код. Формат для передачи: `YANDI-GROUP-1:<номер группы>:<КОД>`; принимается и «номер:КОД».
class GroupInvite {
  final String gid, code;
  const GroupInvite(this.gid, this.code);

  static GroupInvite? tryParse(String raw) {
    var s = raw.trim();
    if (s.startsWith('YANDI-GROUP-1:')) s = s.substring('YANDI-GROUP-1:'.length);
    final m = RegExp(r'^([0-9a-fA-F]{32})[:\s]+([A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4})$').firstMatch(s);
    if (m == null) return null;
    return GroupInvite(m.group(1)!.toLowerCase(), m.group(2)!.toUpperCase());
  }

  String encode() => 'YANDI-GROUP-1:$gid:$code';
}

/// Приглашение, отправленное контакту обычным зашифрованным сообщением: `\u0001yandi-gi:{"g":номер,"c":код,"n":название}`.
class GroupInviteMsg {
  static const marker = '\u0001yandi-gi:';
  final GroupInvite invite;
  final String name;
  const GroupInviteMsg(this.invite, this.name);

  static String encode(String gid, String code, String name) => '$marker${jsonEncode({'g': gid, 'c': code, 'n': name})}';

  static GroupInviteMsg? tryParse(String text) {
    if (!text.startsWith(marker)) return null;
    try {
      final j = jsonDecode(text.substring(marker.length));
      if (j is! Map) return null;
      final code = '${j['c'] ?? ''}';
      // открытая группа: кода нет, вступают сразу; иначе нужен корректный код
      final inv = code.isEmpty
          ? (RegExp(r'^[0-9a-fA-F]{32}$').hasMatch('${j['g']}') ? GroupInvite('${j['g']}'.toLowerCase(), '') : null)
          : GroupInvite.tryParse('${j['g']}:$code');
      if (inv == null) return null;
      final n = '${j['n'] ?? ''}'.replaceAll(RegExp(r'[\x00-\x1f]'), '').trim();
      return GroupInviteMsg(inv, n.length > 64 ? n.substring(0, 64) : n);
    } catch (_) {
      return null;
    }
  }
}
