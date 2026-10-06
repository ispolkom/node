import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/notification_service.dart';
import '../theme.dart';
import 'chat_screen.dart';
import 'files_screen.dart';
import 'settings_screen.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});
  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      context.read<AppState>().refreshContacts();
      _handlePendingNotification();
    });
  }

  void _handlePendingNotification() {
    final peerId = NotificationService.pendingOpenPeerId;
    if (peerId == null) return;
    NotificationService.pendingOpenPeerId = null;
    Navigator.push(context, MaterialPageRoute(
      builder: (_) => ChatScreen(
        peerId: peerId,
        title:  peerId.length > 12 ? peerId.substring(0, 12) : peerId,
      ),
    ));
  }

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();

    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        elevation: 0,
        title: Row(
          children: [
            Image.asset('assets/images/logo.png', height: 28),
            const SizedBox(width: 10),
            const Text('YANDI', style: TextStyle(
              color: AppTheme.accent,
              fontWeight: FontWeight.bold,
              letterSpacing: 2,
            )),
            const SizedBox(width: 8),
            Container(
              width: 8, height: 8,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: state.nodeOnline ? Colors.greenAccent : Colors.redAccent,
              ),
            ),
          ],
        ),
        actions: [
          // VPN toggle
          GestureDetector(
            onTap: () => state.toggleVpn(),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12),
              child: Row(
                children: [
                  Icon(
                    state.vpnRunning ? Icons.vpn_lock : Icons.vpn_lock_outlined,
                    color: state.vpnRunning ? AppTheme.accent : AppTheme.textSecondary,
                  ),
                  const SizedBox(width: 4),
                  Text(
                    state.vpnRunning ? 'VPN ON' : 'VPN',
                    style: TextStyle(
                      color: state.vpnRunning ? AppTheme.accent : AppTheme.textSecondary,
                      fontSize: 12,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                ],
              ),
            ),
          ),
          IconButton(
            icon: const Icon(Icons.settings, color: AppTheme.textSecondary),
            onPressed: () => Navigator.push(context,
                MaterialPageRoute(builder: (_) => const SettingsScreen())),
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton(
        backgroundColor: AppTheme.accent,
        foregroundColor: Colors.black,
        onPressed: () => _showAddContactDialog(context, state),
        child: const Icon(Icons.person_add),
      ),
      body: state.contacts.isEmpty
          ? Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.people_outline, color: AppTheme.textSecondary, size: 64),
                  const SizedBox(height: 16),
                  const Text('Нет контактов', style: TextStyle(color: AppTheme.textSecondary)),
                  const SizedBox(height: 8),
                  TextButton.icon(
                    onPressed: () => state.refreshContacts(),
                    icon: const Icon(Icons.refresh, color: AppTheme.accent),
                    label: const Text('Обновить', style: TextStyle(color: AppTheme.accent)),
                  ),
                ],
              ),
            )
          : RefreshIndicator(
              color: AppTheme.accent,
              backgroundColor: AppTheme.surface,
              onRefresh: () => state.refreshContacts(),
              child: ListView.builder(
                itemCount: state.contacts.length,
                itemBuilder: (ctx, i) {
                  final c = state.contacts[i];
                  return _ContactTile(
                    peerId:      c.peerId,
                    displayName: c.displayName,
                    online:      c.online,
                    isManual:    c.isManual,
                    onTap: () => Navigator.push(
                      context,
                      MaterialPageRoute(
                        builder: (_) => ChatScreen(peerId: c.peerId, title: c.displayName),
                      ),
                    ),
                    onDelete: c.isManual
                        ? () => state.removeManualContact(c.peerId)
                        : null,
                  );
                },
              ),
            ),
    );
  }

  void _showAddContactDialog(BuildContext context, AppState state) {
    final peerCtrl = TextEditingController();
    final nameCtrl = TextEditingController();
    String? error;

    showDialog(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setLocal) => AlertDialog(
          backgroundColor: AppTheme.surface,
          title: const Text('Добавить контакт', style: TextStyle(color: AppTheme.text)),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: peerCtrl,
                style: const TextStyle(color: AppTheme.text, fontSize: 13),
                decoration: _inputDecor('Peer ID (64 hex символа)'),
                maxLength: 64,
                autocorrect: false,
                enableSuggestions: false,
              ),
              const SizedBox(height: 8),
              TextField(
                controller: nameCtrl,
                style: const TextStyle(color: AppTheme.text),
                decoration: _inputDecor('Имя (необязательно)'),
              ),
              if (error != null) ...[
                const SizedBox(height: 8),
                Text(error!, style: const TextStyle(color: Colors.redAccent, fontSize: 12)),
              ],
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary)),
            ),
            TextButton(
              onPressed: () {
                final pid = peerCtrl.text.trim().toLowerCase();
                if (pid.length != 64 || !RegExp(r'^[0-9a-f]+$').hasMatch(pid)) {
                  setLocal(() => error = 'Peer ID должен быть 64 hex-символа');
                  return;
                }
                Navigator.pop(ctx);
                state.addManualContact(pid, nameCtrl.text.trim());
              },
              child: const Text('Добавить', style: TextStyle(color: AppTheme.accent)),
            ),
          ],
        ),
      ),
    );
  }

  InputDecoration _inputDecor(String hint) => InputDecoration(
    hintText: hint,
    hintStyle: const TextStyle(color: AppTheme.textSecondary, fontSize: 13),
    filled: true,
    fillColor: AppTheme.bg,
    counterStyle: const TextStyle(color: AppTheme.textSecondary, fontSize: 10),
    border: OutlineInputBorder(
      borderRadius: BorderRadius.circular(8),
      borderSide: BorderSide.none,
    ),
  );
}

class _ContactTile extends StatelessWidget {
  final String      peerId;
  final String      displayName;
  final bool        online;
  final bool        isManual;
  final VoidCallback onTap;
  final VoidCallback? onDelete;

  const _ContactTile({
    required this.peerId,
    required this.displayName,
    required this.online,
    required this.isManual,
    required this.onTap,
    this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final tile = InkWell(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        decoration: const BoxDecoration(
          border: Border(bottom: BorderSide(color: AppTheme.divider, width: 0.5)),
        ),
        child: Row(
          children: [
            Stack(
              children: [
                CircleAvatar(
                  radius: 24,
                  backgroundColor: AppTheme.surface,
                  child: Text(
                    displayName.isNotEmpty ? displayName[0].toUpperCase() : '?',
                    style: const TextStyle(
                        color: AppTheme.accent,
                        fontWeight: FontWeight.bold,
                        fontSize: 18),
                  ),
                ),
                Positioned(
                  right: 0, bottom: 0,
                  child: Container(
                    width: 12, height: 12,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: online ? Colors.greenAccent : Colors.grey,
                      border: Border.all(color: AppTheme.bg, width: 2),
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Text(displayName, style: const TextStyle(
                          color: AppTheme.text,
                          fontWeight: FontWeight.w600,
                          fontSize: 15)),
                      if (isManual) ...[
                        const SizedBox(width: 6),
                        const Icon(Icons.person, size: 12, color: AppTheme.accent),
                      ],
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    online ? 'онлайн' : 'офлайн',
                    style: TextStyle(
                      color: online ? Colors.greenAccent : AppTheme.textSecondary,
                      fontSize: 12,
                    ),
                  ),
                ],
              ),
            ),
            const Icon(Icons.chevron_right, color: AppTheme.textSecondary),
          ],
        ),
      ),
    );

    if (onDelete == null) return tile;

    // Свайп влево — удалить ручной контакт
    return Dismissible(
      key: ValueKey(peerId),
      direction: DismissDirection.endToStart,
      background: Container(
        alignment: Alignment.centerRight,
        padding: const EdgeInsets.only(right: 20),
        color: Colors.redAccent.withOpacity(0.8),
        child: const Icon(Icons.delete_outline, color: Colors.white),
      ),
      confirmDismiss: (_) async {
        return await showDialog<bool>(
          context: context,
          builder: (_) => AlertDialog(
            backgroundColor: AppTheme.surface,
            title: const Text('Удалить контакт?',
                style: TextStyle(color: AppTheme.text)),
            content: Text('$displayName будет удалён из списка.',
                style: const TextStyle(color: AppTheme.textSecondary)),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('Отмена',
                    style: TextStyle(color: AppTheme.textSecondary)),
              ),
              TextButton(
                onPressed: () => Navigator.pop(context, true),
                child: const Text('Удалить',
                    style: TextStyle(color: Colors.redAccent)),
              ),
            ],
          ),
        ) ?? false;
      },
      onDismissed: (_) => onDelete!(),
      child: tile,
    );
  }
}
