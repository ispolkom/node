import '../transport/api.dart';
import 'shell_repository.dart';

class NetworkRepository {
  final Api api;
  NetworkRepository(this.api);
  String short(String id) {
    if (!RegExp(r'^[a-fA-F0-9]{16}$').hasMatch(id)) {
      throw const FormatException('Short ID: 16 шестнадцатеричных символов');
    }
    return id.toLowerCase();
  }

  Future<List<Map<String, dynamic>>> gateways() async =>
      ShellRepository.list(await api.request('/api/gateways'), 'gateways');
  Future<void> save(
    String name,
    String id,
    String country, {
    String? key,
  }) async {
    if (name.trim().isEmpty ||
        name.runes.length > 128 ||
        !const ['RU', 'US', 'NL', 'DE', 'FR', 'GB'].contains(country)) {
      throw const FormatException('Проверьте имя и страну шлюза');
    }
    final body = <String, dynamic>{
      'name': name.trim(),
      'short_id': short(id),
      'country': country,
    };
    if (key != null) body['id'] = key;
    await api.request('/api/gateways', body: body);
    // CRUD daemon swallows disk errors: confirm observed contents by GET.
    final rows = await gateways();
    if (!rows.any(
      (r) =>
          (key == null || r['id'] == key) &&
          r['name'] == name.trim() &&
          r['short_id'] == short(id) &&
          r['country'] == country,
    )) {
      throw const ApiFailure('business');
    }
  }

  Future<void> remove(String id) async {
    await api.request('/api/gateways', method: 'DELETE', body: {'id': id});
    if ((await gateways()).any((r) => r['id'] == id))
      throw const ApiFailure('business');
  }

  Future<Map<String, dynamic>> proxy(String family) async {
    final value = await api.request('/api/$family/status');
    if (value['active'] is! bool) throw const ApiFailure('protocol');
    ShellRepository.list(value, 'proxies');
    return value;
  }

  Future<Map<String, dynamic>> proxyAction(
    String family,
    String action,
    String id,
  ) => api.request(
    '/api/$family/$action/${ShellRepository(api).segment(id)}',
    method: 'POST',
  );
}
