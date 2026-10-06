import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';

class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final pair  = state.pair;

    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: const Text('Настройки', style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: ListView(
        children: [
          // Нода
          _Section(title: 'Нода'),
          _InfoTile(label: 'Хост', value: pair?.host ?? '—'),
          _InfoTile(label: 'Порт', value: pair?.port.toString() ?? '—'),
          _InfoTile(label: 'Node ID', value: state.myNodeId?.substring(0, 16) ?? '—'),
          _InfoTile(
            label: 'Статус',
            value: state.nodeOnline ? 'Онлайн' : 'Офлайн',
            valueColor: state.nodeOnline ? Colors.greenAccent : Colors.redAccent,
          ),

          // Прокси
          _Section(title: 'Прокси'),
          _InfoTile(
            label: 'SOCKS5',
            value: state.proxyHost != null
                ? '${state.proxyHost}:${state.proxyPort}'
                : 'Не запущен (включите в веб-интерфейсе ноды)',
          ),
          ListTile(
            tileColor: AppTheme.surface,
            title: const Text('VPN через YANDI', style: TextStyle(color: AppTheme.text)),
            subtitle: Text(
              state.vpnRunning ? 'Включён — трафик идёт через YANDI' : 'Выключен',
              style: TextStyle(
                  color: state.vpnRunning ? Colors.greenAccent : AppTheme.textSecondary),
            ),
            trailing: Switch(
              value: state.vpnRunning,
              activeColor: AppTheme.accent,
              onChanged: state.proxyHost != null || !state.vpnRunning
                  ? (_) => state.toggleVpn()
                  : null,
            ),
          ),

          // Аккаунт
          _Section(title: 'Устройство'),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.logout, color: Colors.redAccent),
            title: const Text('Отвязать устройство',
                style: TextStyle(color: Colors.redAccent)),
            onTap: () async {
              final confirm = await showDialog<bool>(
                context: context,
                builder: (ctx) => AlertDialog(
                  backgroundColor: AppTheme.surface,
                  title: const Text('Отвязать?', style: TextStyle(color: AppTheme.text)),
                  content: const Text('Токен будет удалён. Потребуется повторный паринг.',
                      style: TextStyle(color: AppTheme.textSecondary)),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(ctx, false),
                        child: const Text('Отмена')),
                    TextButton(
                        onPressed: () => Navigator.pop(ctx, true),
                        child: const Text('Отвязать',
                            style: TextStyle(color: Colors.redAccent))),
                  ],
                ),
              );
              if (confirm == true && context.mounted) {
                final nodeId = context.read<AppState>().activeNode?.id;
                if (nodeId != null) await context.read<AppState>().unpairNode(nodeId);
                if (context.mounted) {
                  Navigator.of(context).pushNamedAndRemoveUntil('/pair', (_) => false);
                }
              }
            },
          ),
        ],
      ),
    );
  }
}

class _Section extends StatelessWidget {
  final String title;
  const _Section({required this.title});

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.fromLTRB(16, 20, 16, 6),
        child: Text(title.toUpperCase(),
            style: const TextStyle(color: AppTheme.accent, fontSize: 11,
                fontWeight: FontWeight.bold, letterSpacing: 1.2)),
      );
}

class _InfoTile extends StatelessWidget {
  final String  label;
  final String  value;
  final Color?  valueColor;
  const _InfoTile({required this.label, required this.value, this.valueColor});

  @override
  Widget build(BuildContext context) => ListTile(
        tileColor: AppTheme.surface,
        title: Text(label, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
        trailing: Text(value,
            style: TextStyle(color: valueColor ?? AppTheme.text, fontSize: 13)),
      );
}
