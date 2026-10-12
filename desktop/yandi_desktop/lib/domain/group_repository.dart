import 'dart:convert';
import '../transport/api.dart';
import 'shell_repository.dart';

class GroupRepository {
  final Api api;
  GroupRepository(this.api);
  String path(String id, String action) {
    if (!RegExp(r'^[a-f0-9]{32}$').hasMatch(id)) {
      throw const ApiFailure('protocol');
    }
    return '/api/groups/$id/$action';
  }

  Future<Map<String, dynamic>> groups() => api.request('/api/groups');
  Future<Map<String, dynamic>> info(String id) async {
    final result = await api.request(path(id, 'info'));
    validateGroup(result);
    ShellRepository.list(result, 'members');
    return result;
  }

  void validateGroup(Map<String, dynamic> result) {
    if (result['group'] is! Map<String, dynamic> ||
        !const [
          'owner',
          'admin',
          'moderator',
          'member',
        ].contains(result['group']['role'])) {
      throw const ApiFailure('protocol');
    }
  }

  Future<Map<String, dynamic>> act(
    String id,
    String action, [
    Map<String, dynamic> body = const {},
  ]) => api.request(path(id, action), body: body);
  Future<String> create(
    String name,
    String topic,
    String about,
    String kind,
  ) async {
    final result = await api.request(
      '/api/groups/create',
      body: {
        'name': name.trim(),
        'topic': topic.trim(),
        'about': about.trim(),
        'kind': kind,
      },
    );
    final id = result['id'];
    if (id is! String) throw const ApiFailure('protocol');
    path(id, 'info');
    return id;
  }

  Future<void> send(String id, String text) async {
    if (text.trim().isEmpty || utf8.encode(text.trim()).length > 8000) {
      throw const ApiFailure('business');
    }
    await act(id, 'send', {'text': text});
  }

  // No cursor: a complete snapshot removes deleted/denied entries.
  // Retain only display fields, never attachment bodies or keys.
  Future<Map<String, dynamic>> feed(String id, {DateTime? now}) async {
    final result = await api.request(path(id, 'messages'));
    validateGroup(result);
    if (result['has_key'] is! bool || result['deny'] is! List) {
      throw const ApiFailure('protocol');
    }
    final source = ShellRepository.list(result, 'messages');
    if (source.any(
      (m) => m['seq'] is! num || m['ts'] is! num || m['kind'] is! num,
    )) {
      throw const ApiFailure('protocol');
    }
    final today = (now ?? DateTime.now()).toUtc();
    final cutoff = DateTime.utc(
      today.year - 1,
      today.month,
      today.month == 2 && today.day == 29 ? 28 : today.day,
      today.hour,
      today.minute,
      today.second,
      today.millisecond,
      today.microsecond,
    ).millisecondsSinceEpoch;
    final deny = result['deny'] is List ? result['deny'] as List : const [];
    final messages = source
        .where(
          (m) =>
              m['ts'] is num &&
              (m['ts'] as num) >= cutoff &&
              m['deleted'] != true &&
              m['kind'] != 7 &&
              !deny.contains(m['kind']),
        )
        .map(
          (m) => <String, dynamic>{
            'seq': m['seq'],
            'ts': m['ts'],
            'from': m['from'],
            'author': m['author'],
            'mine': m['mine'],
            'kind': m['kind'],
            'text': m['kind'] == 1 || m['kind'] == 6
                ? (m['body'] is Map ? m['body']['t'] : '')
                : null,
          },
        )
        .toList();
    messages.sort((a, b) {
      final order = (a['ts'] as num).compareTo(b['ts'] as num);
      return order != 0 ? order : (a['seq'] as num).compareTo(b['seq'] as num);
    });
    return {
      for (final key in ['group', 'has_key', 'problem', 'damaged', 'deny'])
        key: result[key],
      'messages': messages,
    };
  }

  static bool canTouch(String role, String target) => switch (role) {
    'owner' => target != 'owner',
    'admin' => target == 'member' || target == 'moderator',
    'moderator' => target == 'member',
    _ => false,
  };
}
