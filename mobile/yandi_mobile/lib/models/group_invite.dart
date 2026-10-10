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
