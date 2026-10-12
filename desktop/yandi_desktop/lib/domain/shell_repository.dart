import '../transport/api.dart';

/// All persistence, card validation and cryptography remain in the daemon.
class ShellRepository {
  final Api api;
  ShellRepository(this.api);

  String segment(String value) {
    if (!RegExp(r'^[a-zA-Z0-9_-]{1,128}$').hasMatch(value)) {
      throw const ApiFailure('protocol');
    }
    return Uri.encodeComponent(value);
  }

  Future<List<Map<String, dynamic>>> contacts() async {
    final result = await api.request('/api/contacts');
    return list(result, 'contacts');
  }

  Future<Map<String, dynamic>> trusted() => api.request('/api/peers/trusted');

  Future<void> addCard(
    String card,
    String name, {
    bool Function()? isActive,
  }) async {
    final added = await api.request(
      '/api/peers/trusted',
      body: {'card': card.trim(), 'name': name.trim()},
    );
    final id = added['id'];
    if (id is! String || !RegExp(r'^[a-f0-9]{64}$').hasMatch(id)) {
      throw const ApiFailure('protocol');
    }
    if (isActive != null && !isActive()) return;
    // Card trust and contact saving are two separate daemon operations.
    await api.request(
      '/api/contacts',
      body: {'name': name.trim(), 'short_id': id.substring(0, 16)},
    );
  }

  Future<void> rename(Map<String, dynamic> contact, String name) async {
    await api.request(
      '/api/contacts',
      body: {
        'id': contact['id'],
        'short_id': contact['short_id'],
        'name': name.trim(),
      },
    );
  }

  Future<void> remove(String id) async {
    await api.request('/api/contacts', method: 'DELETE', body: {'id': id});
  }

  Future<List<Map<String, dynamic>>> history(
    String peer, {
    DateTime? now,
  }) async {
    final result = await api.request('/api/chat/history/${segment(peer)}');
    final today = (now ?? DateTime.now()).toUtc();
    // Calendar year, clamping February 29 to February 28.
    final day = today.month == 2 && today.day == 29 ? 28 : today.day;
    final cutoff = DateTime.utc(
      today.year - 1,
      today.month,
      day,
      today.hour,
      today.minute,
      today.second,
      today.millisecond,
      today.microsecond,
    ).millisecondsSinceEpoch;
    final messages = list(result, 'messages')
        .where(
          (m) => m['timestamp'] is num && (m['timestamp'] as num) >= cutoff,
        )
        .toList();
    messages.sort((a, b) {
      final order = (a['timestamp'] as num).compareTo(b['timestamp'] as num);
      return order != 0 ? order : '${a['msg_id']}'.compareTo('${b['msg_id']}');
    });
    return messages;
  }

  Future<void> send(String peer, String text) async {
    await api.request('/api/chat/send/${segment(peer)}', body: {'text': text});
  }

  Future<Map<String, dynamic>> profile() async {
    final result = await api.request('/api/profile/me');
    if (result['profile'] is! Map<String, dynamic>) {
      throw const ApiFailure('protocol');
    }
    return result['profile'] as Map<String, dynamic>;
  }

  Future<void> saveProfile(String name) async {
    await api.request(
      '/api/profile/me',
      method: 'PUT',
      body: {'display_name': name},
    );
  }

  static List<Map<String, dynamic>> list(
    Map<String, dynamic> result,
    String key,
  ) {
    if (result[key] is! List ||
        (result[key] as List).any((v) => v is! Map<String, dynamic>)) {
      throw const ApiFailure('protocol');
    }
    return (result[key] as List).cast<Map<String, dynamic>>();
  }
}
