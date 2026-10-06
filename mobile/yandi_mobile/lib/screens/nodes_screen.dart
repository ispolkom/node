import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../models/trusted_node.dart';
import '../theme.dart';
import 'pair_screen.dart';

class NodesScreen extends StatelessWidget {
  const NodesScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();

    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        elevation: 0,
        title: const Text('Ноды', style: TextStyle(
          color: AppTheme.text, fontWeight: FontWeight.bold)),
        iconTheme: const IconThemeData(color: AppTheme.textSecondary),
      ),
      floatingActionButton: FloatingActionButton(
        backgroundColor: AppTheme.accent,
        foregroundColor: Colors.black,
        onPressed: () => Navigator.push(context,
            MaterialPageRoute(builder: (_) => const PairScreen())),
        child: const Icon(Icons.add),
      ),
      body: state.nodes.isEmpty
          ? Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.hub_outlined,
                      color: AppTheme.textSecondary, size: 64),
                  const SizedBox(height: 16),
                  const Text('Нет подключённых нод',
                      style: TextStyle(color: AppTheme.textSecondary)),
                  const SizedBox(height: 8),
                  TextButton.icon(
                    onPressed: () => Navigator.push(context,
                        MaterialPageRoute(builder: (_) => const PairScreen())),
                    icon: const Icon(Icons.qr_code_scanner, color: AppTheme.accent),
                    label: const Text('Добавить ноду',
                        style: TextStyle(color: AppTheme.accent)),
                  ),
                ],
              ),
            )
          : ListView.builder(
              itemCount: state.nodes.length,
              itemBuilder: (ctx, i) {
                final node = state.nodes[i];
                return _NodeTile(
                  node:        node,
                  isActive:    state.activeNode?.id == node.id,
                  onSetActive: () => state.setPreferredNode(node.id),
                  onDelete:    () => _confirmDelete(ctx, state, node),
                );
              },
            ),
    );
  }

  void _confirmDelete(
      BuildContext context, AppState state, TrustedNode node) {
    showDialog(
      context: context,
      builder: (_) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: const Text('Удалить ноду?',
            style: TextStyle(color: AppTheme.text)),
        content: Text('${node.name} будет удалена.',
            style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Отмена',
                style: TextStyle(color: AppTheme.textSecondary)),
          ),
          TextButton(
            onPressed: () {
              Navigator.pop(context);
              state.removeNode(node.id);
            },
            child: const Text('Удалить',
                style: TextStyle(color: Colors.redAccent)),
          ),
        ],
      ),
    );
  }
}

class _NodeTile extends StatelessWidget {
  final TrustedNode  node;
  final bool         isActive;
  final VoidCallback onSetActive;
  final VoidCallback onDelete;

  const _NodeTile({
    required this.node,
    required this.isActive,
    required this.onSetActive,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: AppTheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(
          color: isActive ? AppTheme.accent : Colors.transparent,
          width: 1.5,
        ),
      ),
      child: Row(
        children: [
          Icon(
            isActive ? Icons.hub : Icons.hub_outlined,
            color: isActive ? AppTheme.accent : AppTheme.textSecondary,
            size: 28,
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(children: [
                  Text(node.name,
                      style: const TextStyle(
                          color: AppTheme.text,
                          fontWeight: FontWeight.w600,
                          fontSize: 15)),
                  if (isActive) ...[
                    const SizedBox(width: 8),
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 6, vertical: 2),
                      decoration: BoxDecoration(
                        color: AppTheme.accent.withOpacity(0.15),
                        borderRadius: BorderRadius.circular(4),
                      ),
                      child: const Text('активна',
                          style: TextStyle(
                              color: AppTheme.accent, fontSize: 10)),
                    ),
                  ],
                ]),
                const SizedBox(height: 4),
                Text('${node.host}:${node.port}',
                    style: const TextStyle(
                        color: AppTheme.textSecondary, fontSize: 12)),
                if (node.pingMs < 999)
                  Text('ping ${node.pingMs} ms  •  load ${(node.loadFactor * 100).toStringAsFixed(0)}%',
                      style: const TextStyle(
                          color: AppTheme.textSecondary, fontSize: 11)),
              ],
            ),
          ),
          PopupMenuButton<String>(
            color: AppTheme.surface,
            icon: const Icon(Icons.more_vert, color: AppTheme.textSecondary),
            onSelected: (v) {
              if (v == 'set') onSetActive();
              if (v == 'del') onDelete();
            },
            itemBuilder: (_) => [
              const PopupMenuItem(value: 'set',
                  child: Text('Сделать активной',
                      style: TextStyle(color: AppTheme.text))),
              const PopupMenuItem(value: 'del',
                  child: Text('Удалить',
                      style: TextStyle(color: Colors.redAccent))),
            ],
          ),
        ],
      ),
    );
  }
}
