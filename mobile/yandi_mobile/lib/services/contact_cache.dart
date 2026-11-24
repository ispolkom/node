import 'dart:convert';
import '../models/contact.dart';
import '../models/contact_merge.dart';

/// Encrypted by the supplied vault store; snapshots belong to one node.
/// Cached roles describe contacts and never grant permissions.
class ContactCache {
  final Future<String?> Function(String) read;
  final Future<void> Function(String, String) write;
  ContactCache({required this.read, required this.write});

  Future<List<Contact>> load(String nodeId, Map<String, String> savedNames,
      bool Function(String) isBlocked) async {
    final cached = <Contact>[];
    try {
      final rows = jsonDecode(await read('contacts_snapshot_$nodeId') ?? '[]') as List;
      for (final row in rows) {
        final m = row as Map;
        cached.add(Contact(
          peerId: m['peer_id'] as String,
          displayName: m['name'] as String,
          online: false,
          isManual: m['manual'] == true,
          saved: m['saved'] == true,
          lastActivityMs: (m['last_activity_ms'] as num?)?.toInt() ?? 0,
          role: m['role'] as String? ?? '',
        ));
      }
    } catch (_) {
      // Local saved contacts remain available if a snapshot is unavailable.
      cached.clear();
    }
    return mergeContacts(fromNode: cached, catalog: const [],
        savedNames: savedNames, isBlocked: isBlocked);
  }

  Future<void> save(String nodeId, List<Contact> contacts) async {
    final value = jsonEncode([for (final c in contacts) {
      'peer_id': c.peerId, 'name': c.displayName, 'manual': c.isManual,
      'saved': c.saved, 'role': c.role, 'last_activity_ms': c.lastActivityMs,
    }]);
    final key = 'contacts_snapshot_$nodeId';
    if (await read(key) != value) await write(key, value);
  }
}
