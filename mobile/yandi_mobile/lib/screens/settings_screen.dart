import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:file_picker/file_picker.dart';
import '../services/app_state.dart';
import '../services/vault_lock.dart';
import 'vault_screens.dart';
import '../theme.dart';
import '../widgets/avatar_circle.dart';

const _emojis = ['😀','😎','🤓','🥳','😇','🤠','😺','🐱','🐶','🦊','🐼','🦁','🐯','🐸','🐵','🦉','🐧','🐢','🦄','🐙',
  '🌞','🌙','⭐','🔥','🌈','🍀','🌸','🌵','🍉','🍕','🎸','⚽','🚀','🛡️','🔑','💎','🎯','🧩','🪐','⚓'];

class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  /// Выбор картинки: фото из памяти, эмодзи или убрать. Системные «Поделиться/галерея» не используются для отправки — файл читается один раз
  /// и сразу пересобирается в маленький PNG, исходник нигде не сохраняется.
  Future<void> _avatarSheet(BuildContext context) async {
    final state = context.read<AppState>();
    final choice = await showModalBottomSheet<String>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(leading: const Icon(Icons.photo_outlined), title: const Text('Выбрать фото'), onTap: () => Navigator.pop(ctx, 'photo')),
          ListTile(leading: const Icon(Icons.emoji_emotions_outlined), title: const Text('Выбрать эмодзи'), onTap: () => Navigator.pop(ctx, 'emoji')),
          if (state.myAvatarBytes != null)
            ListTile(leading: const Icon(Icons.delete_outline, color: Colors.redAccent), title: const Text('Убрать картинку', style: TextStyle(color: Colors.redAccent)), onTap: () => Navigator.pop(ctx, 'clear')),
        ]),
      ),
    );
    if (choice == null || !context.mounted) return;
    String? err;
    if (choice == 'clear') {
      await state.clearMyAvatar();
    } else if (choice == 'photo') {
      final res = await FilePicker.platform.pickFiles(type: FileType.image, withData: true);
      if (res == null) return;
      try {
        final bytes = res.files.single.bytes;
        if (bytes == null) return;
        err = await state.setMyAvatarFromImage(bytes);
      } finally {
        for (final file in res.files) {
          if (file.path != null) await state.dropPickerCopy(file.path!);
        }
      }
    } else {
      final emoji = await showDialog<String>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: AppTheme.surface,
          title: const Text('Эмодзи', style: TextStyle(color: AppTheme.text)),
          content: SizedBox(
            width: 300,
            child: GridView.count(
              shrinkWrap: true,
              crossAxisCount: 5,
              children: [for (final e in _emojis) InkWell(onTap: () => Navigator.pop(ctx, e), child: Center(child: Text(e, style: const TextStyle(fontSize: 28))))],
            ),
          ),
        ),
      );
      if (emoji == null) return;
      err = await state.setMyAvatarEmoji(emoji);
    }
    if (err != null && context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
  }

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
          // Профиль
          _Section(title: 'Профиль'),
          ListTile(
            leading: const AvatarCircle.mine(label: 'Я', radius: 28),
            title: const Text('Моя картинка', style: TextStyle(color: AppTheme.text)),
            subtitle: const Text('Её видят только ваши контакты. Метаданные фото не передаются: картинка собирается заново.',
                style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => _avatarSheet(context),
          ),
          SwitchListTile(
            secondary: const Icon(Icons.keyboard_outlined, color: AppTheme.textSecondary),
            title: const Text('Своя клавиатура в чате', style: TextStyle(color: AppTheme.text)),
            subtitle: const Text('Системная клавиатура видит каждое нажатие и может учиться на нём. Своя работает только внутри приложения; '
                'без автозамены и голосового ввода.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            value: state.secureKeyboard,
            activeColor: AppTheme.accent,
            onChanged: (v) => state.setSecureKeyboard(v),
          ),
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
          if (state.nodeError != null)
            _InfoTile(label: 'Ошибка', value: state.nodeError!, valueColor: Colors.redAccent),

          // Прокси
          _Section(title: 'Прокси'),
          _InfoTile(
            label: 'SOCKS5',
            value: state.proxyHost != null
                ? '${state.proxyHost}:${state.proxyPort}'
                : state.proxyUnreachable
                    ? 'Узел не ответил — проверьте связь и откройте настройки снова'
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

          // Защита
          _Section(title: 'Защита'),
          if (state.protectionWarning != null)
            ListTile(
              tileColor: AppTheme.surface,
              leading: const Icon(Icons.warning_amber_rounded, color: Colors.orangeAccent),
              title: const Text('Защита доведена не до конца', style: TextStyle(color: Colors.orangeAccent)),
              subtitle: Text(state.protectionWarning!, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.lock_outline, color: AppTheme.textSecondary),
            title: const Text('Заблокировать сейчас', style: TextStyle(color: AppTheme.text)),
            subtitle: const Text('Иначе приложение блокируется само через минуту после выхода из него.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => VaultLockObserver.instance?.lockNow(),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.password_outlined, color: AppTheme.textSecondary),
            title: const Text('Сменить пароль', style: TextStyle(color: AppTheme.text)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultChangePasswordScreen())),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.delete_forever_outlined, color: Colors.redAccent),
            title: const Text('Удалить всё', style: TextStyle(color: Colors.redAccent)),
            subtitle: const Text('Стереть все данные приложения с этого телефона (нужен пароль).', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultEraseScreen())),
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
