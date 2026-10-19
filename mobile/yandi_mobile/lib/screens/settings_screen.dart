import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:file_picker/file_picker.dart';
import '../services/app_state.dart';
import '../services/vault_lock.dart';
import 'vault_screens.dart';
import 'invite_device_screen.dart';
import 'support_screen.dart';
import '../theme.dart';
import '../widgets/secure_text_field.dart';
import '../widgets/avatar_circle.dart';
import '../l10n.dart';

const _emojis = ['😀','😎','🤓','🥳','😇','🤠','😺','🐱','🐶','🦊','🐼','🦁','🐯','🐸','🐵','🦉','🐧','🐢','🦄','🐙',
  '🌞','🌙','⭐','🔥','🌈','🍀','🌸','🌵','🍉','🍕','🎸','⚽','🚀','🛡️','🔑','💎','🎯','🧩','🪐','⚓'];

class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  Future<void> _nickDialog(BuildContext context) async {
    final state = context.read<AppState>();
    final ctl = TextEditingController(text: state.myNick);
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(tr('Мой ник'), style: TextStyle(color: AppTheme.text)),
        content: SecureTextField(
          key: const ValueKey('nick-field'),
          controller: ctl,
          maxLength: 40,
          autofocus: true,
          style: const TextStyle(color: AppTheme.text),
          decoration: InputDecoration(hintText: tr('Например: Доктор Хаус'), helperText: tr('Его увидят контакты и участники групп. Не указывайте ФИО.'), helperMaxLines: 2),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'))),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Сохранить'))),
        ],
      ),
    );
    final text = ctl.text;
    ctl.dispose();
    if (ok != true || !context.mounted) return;
    final err = await state.setMyNick(text);
    if (err != null && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
    }
  }

  /// Выбор картинки: фото из памяти, эмодзи или убрать. Системные «Поделиться/галерея» не используются для отправки — файл читается один раз
  /// и сразу пересобирается в маленький PNG, исходник нигде не сохраняется.
  Future<void> _avatarSheet(BuildContext context) async {
    final state = context.read<AppState>();
    final choice = await showModalBottomSheet<String>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(leading: const Icon(Icons.photo_outlined), title: Text(tr('Выбрать фото')), onTap: () => Navigator.pop(ctx, 'photo')),
          ListTile(leading: const Icon(Icons.emoji_emotions_outlined), title: Text(tr('Выбрать эмодзи')), onTap: () => Navigator.pop(ctx, 'emoji')),
          if (state.myAvatarBytes != null)
            ListTile(leading: const Icon(Icons.delete_outline, color: Colors.redAccent), title: Text(tr('Убрать картинку'), style: TextStyle(color: Colors.redAccent)), onTap: () => Navigator.pop(ctx, 'clear')),
        ]),
      ),
    );
    if (choice == null || !context.mounted) return;
    String? err;
    if (choice == 'clear') {
      await state.clearMyAvatar();
    } else if (choice == 'photo') {
      // личное фото может раскрыть, кто вы: просим подтвердить осознанно
      final go = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: AppTheme.surface,
          title: Text(tr('Личное фото — риск'), style: TextStyle(color: AppTheme.text)),
          content: Text(
              tr('Личное фото может вас скомпрометировать: по лицу и обстановке вас могут узнать. Надёжнее выбрать эмодзи или нейтральную картинку.\n\n'
              'Метаданные снимка (место, время, модель телефона) будут удалены, картинка пересоберётся заново и сохранится зашифрованной.'),
              style: TextStyle(color: AppTheme.textSecondary)),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена'), style: TextStyle(color: AppTheme.textSecondary))),
            TextButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Всё равно выбрать фото'), style: TextStyle(color: Colors.redAccent))),
          ],
        ),
      );
      if (go != true) return;
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
          title: Text(tr('Эмодзи'), style: TextStyle(color: AppTheme.text)),
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
        title: Text(tr('Настройки'), style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: ListView(
        children: [
          if (state.canInvite || state.myRole == 'invited') ...[
            _Section(title: tr('Приглашения')),
            if (state.canInvite)
              ListTile(
                key: const ValueKey('invite-device'),
                leading: const Icon(Icons.qr_code_2, color: AppTheme.textSecondary),
                title: Text(tr('Пригласить устройство'), style: TextStyle(color: AppTheme.text)),
                subtitle: Text(tr('QR или код для друга: он подключится к этому узлу. Приглашённый приглашать сам не сможет.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                onTap: () => Navigator.of(context).push(MaterialPageRoute(
                  builder: (_) => InviteDeviceScreen(create: state.createDeviceInvite, revoke: state.revokeDeviceInvites, restore: state.activeDeviceInvite),
                )),
              )
            else
              ListTile(
                key: ValueKey('invite-device-locked'),
                leading: Icon(Icons.lock_outline, color: AppTheme.textSecondary),
                title: Text(tr('Приглашать другие устройства'), style: TextStyle(color: AppTheme.text)),
                subtitle: Text(tr('Вас пригласили: право приглашать даёт администратор узла.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              ),
          ],
          _Section(title: tr('Проект')),
          ListTile(
            key: const ValueKey('support-project'),
            leading: const Icon(Icons.favorite_border, color: AppTheme.textSecondary),
            title: Text(tr('Поддержать проект'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Добровольное пожертвование на серверы и разработку (кошелёк проекта, а не узла).'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => SupportScreen(wallets: context.read<AppState>().wallets))),
          ),
          // Профиль
          _Section(title: tr('Профиль')),
          ListTile(
            key: const ValueKey('profile-nick'),
            leading: const Icon(Icons.badge_outlined, color: AppTheme.textSecondary),
            title: Text(tr('Мой ник'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(
              state.myNick.isEmpty ? tr('Не задан. Ник и картинку увидят контакты и участники ваших групп. Не указывайте ФИО и личные фото; ник один на ПК и все ваши телефоны.') : state.myNick,
              style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12),
            ),
            onTap: () => _nickDialog(context),
          ),
          ListTile(
            leading: AvatarCircle.mine(label: tr('Я'), radius: 28),
            title: Text(tr('Моя картинка'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Её увидят контакты и участники ваших групп. Не ставьте личные фото. Метаданные не передаются: картинка собирается заново.'),
                style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => _avatarSheet(context),
          ),
          SwitchListTile(
            secondary: const Icon(Icons.keyboard_outlined, color: AppTheme.textSecondary),
            title: Text(tr('Своя клавиатура в чате'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Системная клавиатура видит каждое нажатие и может учиться на нём. Своя работает только внутри приложения; '
                'без автозамены и голосового ввода.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            value: state.secureKeyboard,
            activeColor: AppTheme.accent,
            onChanged: (v) => state.setSecureKeyboard(v),
          ),
          // Нода
          _Section(title: tr('Нода')),
          _InfoTile(label: 'Node ID', value: state.myNodeId?.substring(0, 16) ?? '—'),
          _InfoTile(
            label: tr('Статус'),
            value: state.nodeOnline ? tr('Онлайн') : tr('Офлайн'),
            valueColor: state.nodeOnline ? Colors.greenAccent : Colors.redAccent,
          ),
          if (state.nodeError != null)
            _InfoTile(label: tr('Ошибка'), value: state.nodeError!, valueColor: Colors.redAccent),

          // Прокси
          _Section(title: tr('Прокси')),
          _InfoTile(
            label: 'SOCKS5',
            value: state.proxyHost != null
                ? '${state.proxyHost}:${state.proxyPort}'
                : state.proxyUnreachable
                    ? tr('Узел не ответил — проверьте связь и откройте настройки снова')
                    : tr('Не запущен (включите в веб-интерфейсе ноды)'),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            title: Text(tr('VPN через YANDI'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(
              state.vpnRunning ? tr('Включён — трафик идёт через YANDI') : tr('Выключен'),
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
          _Section(title: tr('Защита')),
          if (state.protectionWarning != null)
            ListTile(
              tileColor: AppTheme.surface,
              leading: const Icon(Icons.warning_amber_rounded, color: Colors.orangeAccent),
              title: Text(tr('Защита доведена не до конца'), style: TextStyle(color: Colors.orangeAccent)),
              subtitle: Text(state.protectionWarning!, style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.lock_outline, color: AppTheme.textSecondary),
            title: Text(tr('Заблокировать сейчас'), style: TextStyle(color: AppTheme.text)),
            subtitle: Text(tr('Иначе приложение блокируется само через минуту после выхода из него.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => VaultLockObserver.instance?.lockNow(manual: true),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.password_outlined, color: AppTheme.textSecondary),
            title: Text(tr('Сменить пароль'), style: TextStyle(color: AppTheme.text)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultChangePasswordScreen())),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.delete_forever_outlined, color: Colors.redAccent),
            title: Text(tr('Удалить всё'), style: TextStyle(color: Colors.redAccent)),
            subtitle: Text(tr('Стереть все данные приложения с этого телефона (нужен пароль).'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultEraseScreen())),
          ),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.warning_amber_outlined, color: Colors.redAccent),
            title: Text(tr('Экстренное стирание'), style: TextStyle(color: Colors.redAccent)),
            subtitle: Text(tr('Стереть всё сразу, без пароля: нужно ввести слово-подтверждение.'), style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            onTap: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const VaultPanicScreen())),
          ),

          // Аккаунт
          _Section(title: tr('Устройство')),
          ListTile(
            tileColor: AppTheme.surface,
            leading: const Icon(Icons.logout, color: Colors.redAccent),
            title: Text(tr('Отвязать устройство'),
                style: TextStyle(color: Colors.redAccent)),
            onTap: () async {
              final confirm = await showDialog<bool>(
                context: context,
                builder: (ctx) => AlertDialog(
                  backgroundColor: AppTheme.surface,
                  title: Text(tr('Отвязать?'), style: TextStyle(color: AppTheme.text)),
                  content: Text(tr('Токен будет удалён. Потребуется повторный паринг.'),
                      style: TextStyle(color: AppTheme.textSecondary)),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(ctx, false),
                        child: Text(tr('Отмена'))),
                    TextButton(
                        onPressed: () => Navigator.pop(ctx, true),
                        child: Text(tr('Отвязать'),
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
