import 'contact.dart';

/// Список людей телефона: что сообщил узел (контакты владельца или каталог людей узла), плюс то, что человек сохранил у себя.
/// Роль, присутствие и время активности всегда берутся от узла — сохранение или переименование их не стирает (гость остаётся гостем и «в сети»).
List<Contact> mergeContacts({
  required List<Contact> fromNode,
  required List<Map<String, dynamic>> catalog,
  required Map<String, String> savedNames,
  required bool Function(String peerId) isBlocked,
}) {
  final byId = <String, Contact>{};
  for (final c in fromNode) {
    byId[c.peerId] = c;
  }
  for (final u in catalog) {
    final id = u['peer_id'] as String? ?? '';
    if (id.isEmpty) continue;
    // ПК узла — участник (docs/ROADMAP_R9.md §1): у владельца он уже пришёл из контактов, у пользователя и гостя — из каталога
    final isNode = u['is_node'] == true;
    if (isNode && byId.containsKey(id)) continue;
    final nick = (u['nick'] as String? ?? '').trim();
    final name = isNode ? '💻 Компьютер' : nick.isNotEmpty ? nick : (u['name'] as String? ?? (id.length >= 12 ? id.substring(0, 12) : id));
    final fresh = Contact(
      peerId: id,
      displayName: name,
      online: u['online'] == true,
      isManual: false,
      lastActivityMs: (u['last_activity_ms'] as num?)?.toInt() ?? 0,
      role: u['role'] as String? ?? '',
    );
    final old = byId[id];
    // запись из каталога свежее по присутствию и роли; имя из контактов владельца сохраняется
    byId[id] = old == null
        ? fresh
        : Contact(peerId: id, displayName: old.displayName, online: fresh.online, isManual: old.isManual, saved: old.saved, lastActivityMs: fresh.lastActivityMs, role: fresh.role.isNotEmpty ? fresh.role : old.role);
  }
  for (final e in savedNames.entries) {
    final c = byId[e.key];
    if (c == null) {
      // знакомый только по номеру: «в сети», если узел видит его под тем же полным номером (вне каталога — не в сети)
      byId[e.key] = Contact(peerId: e.key, displayName: e.value.isEmpty ? e.key.substring(0, 12) : e.value, online: false, isManual: true);
    } else {
      // имя, которое человек задал сам, сильнее имени с узла; остальное (роль, присутствие) остаётся от узла
      byId[e.key] = Contact(peerId: c.peerId, displayName: e.value.isEmpty ? c.displayName : e.value, online: c.online, isManual: c.isManual, saved: true, lastActivityMs: c.lastActivityMs, role: c.role);
    }
  }
  return [for (final c in byId.values) if (!isBlocked(c.peerId)) c];
}
