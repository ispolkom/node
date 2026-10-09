import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';

/// Чёрный список: заблокированные собеседники. Блок двусторонний и держится в приложении (узел о нём не знает).
class BlacklistScreen extends StatelessWidget {
  const BlacklistScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final blocked = state.blockedContacts;
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: const Text('Чёрный список', style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: blocked.isEmpty
          ? const Center(
              child: Padding(
                padding: EdgeInsets.all(32),
                child: Text(
                  'Список пуст.\nЗаблокированные собеседники не могут писать и звонить вам, а вы — им.',
                  textAlign: TextAlign.center,
                  style: TextStyle(color: AppTheme.textSecondary),
                ),
              ),
            )
          : ListView.builder(
              itemCount: blocked.length,
              itemBuilder: (_, i) {
                final e = blocked[i];
                final name = e.value.isNotEmpty ? e.value : e.key.substring(0, 12);
                return ListTile(
                  leading: const CircleAvatar(
                    backgroundColor: AppTheme.surface,
                    child: Icon(Icons.block, color: Colors.redAccent),
                  ),
                  title: Text(name, style: const TextStyle(color: AppTheme.text)),
                  subtitle: Text(e.key.substring(0, 16) + '…',
                      style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
                  trailing: TextButton(
                    onPressed: () => context.read<AppState>().unblockContact(e.key),
                    child: const Text('Разблокировать', style: TextStyle(color: AppTheme.accent)),
                  ),
                );
              },
            ),
    );
  }
}
