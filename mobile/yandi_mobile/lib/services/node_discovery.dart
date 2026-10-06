import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart' as crypto;
import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import '../models/trusted_node.dart';
import 'node_manager.dart';
import 'storage_service.dart';

/// Обнаруживает новые ноды через bootstrap-endpoint /nodes.
/// Используется при старте и в фоновой задаче WorkManager.
class NodeDiscovery {
  static const _discoveryEndpoint = '/nodes';
  static const _mobileInfoEndpoint = '/mobile/info';

  /// Запуск обнаружения при старте приложения.
  static Future<void> discoverOnStartup(NodeManager manager) async {
    // Обновляем метрики существующих нод
    await manager.pingAll();

    // Если нод нет — пробуем bootstrap
    if (manager.nodes.isEmpty) {
      await _fetchBootstrapNodes(manager);
    }
  }

  /// Фоновое обновление (вызывается WorkManager раз в сутки).
  static Future<void> backgroundRefresh(NodeManager manager) async {
    await manager.load();
    await manager.pingAll();

    // Запоминаем время последнего обнаружения
    await StorageService.secureWrite(
      'discovery_last_run',
      DateTime.now().millisecondsSinceEpoch.toString(),
    );
  }

  /// Получить список нод с /mobile/discover на текущей ноде.
  static Future<List<TrustedNode>> discoverFromNode(TrustedNode node) async {
    try {
      final client = _buildClient(node.fingerprint);
      final uri = Uri.parse('${node.httpUrl}/mobile/discover');
      final resp = await client.get(uri,
          headers: {'Authorization': 'Bearer ${node.token}'});
      if (resp.statusCode != 200) return [];

      final list = jsonDecode(resp.body) as List<dynamic>;
      return list
          .cast<Map<String, dynamic>>()
          .map(_nodeFromDiscovery)
          .whereType<TrustedNode>()
          .toList();
    } catch (_) {
      return [];
    }
  }

  static Future<void> _fetchBootstrapNodes(NodeManager manager) async {
    // Bootstrap через публичный endpoint — без аутентификации
    try {
      final resp = await http.get(Uri.parse('https://yandi.network/nodes'))
          .timeout(const Duration(seconds: 10));
      if (resp.statusCode != 200) return;

      final list = jsonDecode(resp.body) as List<dynamic>;
      for (final item in list.cast<Map<String, dynamic>>()) {
        final node = _nodeFromDiscovery(item);
        if (node != null) await manager.addNode(node);
      }
    } catch (_) {}
  }

  static TrustedNode? _nodeFromDiscovery(Map<String, dynamic> json) {
    try {
      return TrustedNode(
        id:          json['id']          as String,
        name:        json['name']        as String? ?? 'Node',
        host:        json['host']        as String,
        port:        json['port']        as int,
        fingerprint: json['fingerprint'] as String? ?? '',
        version:     json['version']     as String? ?? '',
        addedAt:     DateTime.now().millisecondsSinceEpoch,
      );
    } catch (_) {
      return null;
    }
  }

  static http.Client _buildClient(String fingerprint) {
    if (fingerprint.isEmpty) return http.Client();
    final ctx = SecurityContext(withTrustedRoots: false);
    final inner = HttpClient(context: ctx)
      ..badCertificateCallback = (cert, host, port) {
          final fp = crypto.sha256.convert(cert.der).toString();
          return fp.toLowerCase() == fingerprint.toLowerCase();
        };
    return IOClient(inner);
  }
}
