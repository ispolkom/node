import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/trusted_node.dart';
import 'package:yandi_mobile/services/node_manager.dart';

TrustedNode node(String id, {bool preferred = false, int ping = 100}) =>
    TrustedNode(id: id, name: id, host: '$id.example', port: 8444, fingerprint: 'f', pingMs: ping, isPreferred: preferred, addedAt: 0);

void main() {
  test('failover доходит до третьего узла: A → B → C, а не крутится между A и B (P7-7)', () async {
    final m = NodeManager()..setNodesForTest([node('A', preferred: true, ping: 10), node('B', ping: 50), node('C', ping: 200)]);
    final t = DateTime(2026, 10, 16, 12);
    final first = await m.failover(currentId: 'A', now: t);
    expect(first?.id, 'B');
    // службы переехали на B, но «предпочтительный» по-прежнему A; отказал B — следующий C
    final second = await m.failover(currentId: 'B', now: t.add(const Duration(seconds: 5)));
    expect(second?.id, 'C');
    // отказал и C: все недавно отказывали — берём лучший из остальных (A), счёт начинается заново
    final third = await m.failover(currentId: 'C', now: t.add(const Duration(seconds: 10)));
    expect(third?.id, 'A');
  });

  test('один узел — переключаться некуда', () async {
    final m = NodeManager()..setNodesForTest([node('A')]);
    expect(await m.failover(currentId: 'A'), isNull);
  });
}
