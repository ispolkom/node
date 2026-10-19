import 'dart:convert';
import 'package:http/http.dart' as http;
import '../l10n_core.dart';
import '../models/guest_node.dart';
import '../models/trusted_node.dart';

/// Ошибка входа гостем с готовым текстом для человека.
class GuestPairError implements Exception {
  final String message;
  const GuestPairError(this.message);
  @override
  String toString() => message;
}

/// Вход гостем на узел без кода и ссылки: узел выдаёт токен роли `guest` с ограничениями. Соединение только с сертификатом из списка.
/// [client] — клиент HTTPS с закреплённым отпечатком (в тестах подменяется), [deviceName] — имя устройства на узле.
Future<TrustedNode> guestPair(GuestNode n, {required http.Client client, required String deviceName}) async {
  final base = 'https://${n.host}:${n.port}';
  try {
    final res = await client
        .post(Uri.parse('$base/mobile/pair/guest'), headers: {'Content-Type': 'application/json'}, body: jsonEncode({'device_name': deviceName}))
        .timeout(const Duration(seconds: 10));
    if (res.statusCode == 503) throw GuestPairError(tr('На этом узле сейчас нет места для гостей. Попробуйте другой узел или позже.'));
    if (res.statusCode == 403 || res.statusCode == 404) throw GuestPairError(tr('Этот узел не принимает гостей. Попросите код или ссылку у администратора.'));
    if (res.statusCode == 429) throw GuestPairError(tr('Слишком много попыток. Подождите и повторите.'));
    if (res.statusCode != 200) throw GuestPairError(tr('Узел отказал во входе гостем.'));
    final body = jsonDecode(res.body) as Map<String, dynamic>;
    final token = body['token'];
    if (token is! String || token.isEmpty || body['role'] != 'guest') throw GuestPairError(tr('Узел ответил неверно: вход гостем отменён.'));
    final infoRes = await client.get(Uri.parse('$base/mobile/info'), headers: {'Authorization': 'Bearer $token'}).timeout(const Duration(seconds: 10));
    final info = infoRes.statusCode == 200 ? jsonDecode(infoRes.body) as Map<String, dynamic> : <String, dynamic>{};
    final nodeId = info['node_id'] as String? ?? '';
    return TrustedNode(
      id: nodeId.isNotEmpty ? nodeId : '${n.host}:${n.port}',
      name: info['name'] as String? ?? n.name,
      host: n.host,
      port: n.port,
      fingerprint: n.fingerprint,
      token: token,
      addedAt: DateTime.now().millisecondsSinceEpoch,
    );
  } on GuestPairError {
    rethrow;
  } catch (_) {
    throw GuestPairError(tr('Не удалось связаться с узлом. Проверьте интернет или выберите другой узел.'));
  } finally {
    client.close();
  }
}
