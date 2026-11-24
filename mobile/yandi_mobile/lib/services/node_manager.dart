import '../models/trusted_node.dart';
import 'storage_service.dart';
import 'api_service.dart';

/// Управляет списком доверенных нод.
/// Загружает/сохраняет из SQLite, пингует, выбирает лучшую ноду.
class NodeManager {
  List<TrustedNode> _nodes = [];

  List<TrustedNode> get nodes       => List.unmodifiable(_nodes);
  List<TrustedNode> get rankedNodes => [..._nodes]..sort((a, b) => b.rating.compareTo(a.rating));

  TrustedNode? get activeNode {
    if (_nodes.isEmpty) return null;
    final preferred = _nodes.where((n) => n.isPreferred).toList();
    if (preferred.isNotEmpty) {
      preferred.sort((a, b) => b.rating.compareTo(a.rating));
      return preferred.first;
    }
    return rankedNodes.firstOrNull;
  }

  TrustedNode? get preferredNode => activeNode;

  /// Только для тестов: подставить список узлов без базы.
  void setNodesForTest(List<TrustedNode> nodes) => _nodes = List.of(nodes);

  /// Блокировка / стирание: список узлов из памяти убираем.
  void clear() => _nodes = [];

  Future<void> load() async {
    _nodes = await StorageService.loadNodes();
  }

  Future<void> addNode(TrustedNode node) async {
    await StorageService.saveNode(node);
    _nodes.removeWhere((n) => n.id == node.id);
    _nodes.add(node);
  }

  Future<void> removeNode(String id) async {
    await StorageService.deleteNode(id);
    _nodes.removeWhere((n) => n.id == id);
  }

  Future<void> setPreferred(String id) async {
    for (final node in _nodes) {
      if (node.isPreferred || node.id == id) {
        final updated = node.copyWith(isPreferred: node.id == id);
        await StorageService.updateNodeMetrics(updated);
      }
    }
    _nodes = _nodes
        .map((n) => n.copyWith(isPreferred: n.id == id))
        .toList();
  }

  /// Пингует все ноды параллельно, обновляет метрики в БД.
  Future<void> pingAll() async {
    await Future.wait(_nodes.map((node) async {
      try {
        final api = ApiService(node);
        final info = await api.getInfo();
        final updated = node.copyWith(
          pingMs:      info['ping_ms']      as int?    ?? node.pingMs,
          loadFactor:  (info['load_factor'] as num?)?.toDouble() ?? node.loadFactor,
          uptimeHours: info['uptime_hours'] as int?    ?? node.uptimeHours,
          version:     info['version']      as String? ?? node.version,
          lastSeen:    DateTime.now(),
        );
        await StorageService.updateNodeMetrics(updated);
        final idx = _nodes.indexWhere((n) => n.id == node.id);
        if (idx >= 0) _nodes[idx] = updated;
      } catch (_) {
        // Нода недоступна — оставляем старые метрики
      }
    }));
  }

  Future<TrustedNode?> selectBest() async {
    if (_nodes.isEmpty) return null;
    await pingAll();
    return activeNode;
  }

  final Map<String, DateTime> _failedAt = {};

  /// Failover: следующая лучшая нода кроме той, с которой связь только что потеряна ([currentId] — фактически подключённая, а не
  /// «предпочтительная»), и кроме недавно отказавших: цепочка A → B → C доходит до C, а не крутится между A и B.
  Future<TrustedNode?> failover({String? currentId, DateTime? now}) async {
    final t = now ?? DateTime.now();
    final lost = currentId ?? activeNode?.id;
    if (lost != null) _failedAt[lost] = t;
    _failedAt.removeWhere((_, at) => t.difference(at) > const Duration(minutes: 2));
    final fresh = rankedNodes.where((n) => n.id != lost && !_failedAt.containsKey(n.id)).toList();
    if (fresh.isNotEmpty) return fresh.first;
    // все остальные недавно отказывали: пробуем лучший из них (кроме потерянного), список отказов начинается заново
    final others = rankedNodes.where((n) => n.id != lost).toList();
    _failedAt
      ..clear()
      ..[lost ?? ''] = t;
    return others.firstOrNull;
  }
}
