import 'dart:convert';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// Хранилище пары «ключ → строка» (в тестах подменяется памятью).
abstract class InviteBackend {
  Future<String?> read(String k);
  Future<void> write(String k, String v);
  Future<void> delete(String k);
}

class _SecureBackend implements InviteBackend {
  static const _s = FlutterSecureStorage();
  @override
  Future<String?> read(String k) => _s.read(key: k);
  @override
  Future<void> write(String k, String v) => _s.write(key: k, value: v);
  @override
  Future<void> delete(String k) => _s.delete(key: k);
}

/// Приглашения живут час и должны пережить закрытие приложения: у пригласившего — выданное (ссылка, код, PIN), у приглашённого — принятая ссылка,
/// пока он не введёт PIN. Хранится в защищённом хранилище телефона, по истечении срока стирается при первом же чтении.
class InviteStore {
  static InviteBackend backend = _SecureBackend();
  static int Function() nowMs = () => DateTime.now().millisecondsSinceEpoch;
  static const _issued = 'invite.issued', _pending = 'invite.pending';

  static Future<Map<String, dynamic>?> _load(String key) async {
    try {
      final raw = await backend.read(key);
      if (raw == null) return null;
      final m = jsonDecode(raw);
      if (m is Map<String, dynamic> && m['exp'] is int && (m['exp'] as int) > nowMs()) return m;
      await backend.delete(key);
    } catch (_) {
      try {
        await backend.delete(key);
      } catch (_) {}
    }
    return null;
  }

  /// [node] — узел, на котором выдано приглашение: показывать и отзывать его можно только на нём.
  static Future<void> saveIssued({required String node, required String text, required String code, required String pin, required int expMs}) async =>
      backend.write(_issued, jsonEncode({'node': node, 'text': text, 'code': code, 'pin': pin, 'exp': expMs}));

  /// {node, text, code, pin, exp(мс)} живого выданного приглашения или null.
  static Future<Map<String, dynamic>?> issued() => _load(_issued);
  static Future<void> clearIssued() => backend.delete(_issued);

  static Future<void> savePending({required String text, required int expMs}) async => backend.write(_pending, jsonEncode({'text': text, 'exp': expMs}));

  /// {text, exp(мс)} принятой, но ещё не использованной ссылки или null.
  static Future<Map<String, dynamic>?> pending() => _load(_pending);
  static Future<void> clearPending() => backend.delete(_pending);
}
