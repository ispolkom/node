import 'dart:async';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import '../models/contact.dart';
import '../services/app_state.dart';
import '../services/notification_service.dart';
import '../theme.dart';
import '../widgets/secure_text_field.dart';
import 'chat_screen.dart';
import 'add_contact_screen.dart';
import 'qr_scan_screen.dart';
import 'pair_screen.dart';
import 'search_hub.dart';
import '../widgets/secure_keyboard.dart';
import '../widgets/intro_requests.dart';
import '../widgets/complaint_sheet.dart';
import '../widgets/secure_field.dart';
import 'blacklist_screen.dart';
import 'settings_screen.dart';
import 'files_tab.dart';
import 'groups_tab.dart';
import '../widgets/yandi_icons.dart';
import '../widgets/avatar_circle.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key});
  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> with SingleTickerProviderStateMixin {
  late final TabController _tabs;
  final _filesKey = GlobalKey<FilesTabState>();
  final _searchCtrl = TextEditingController();
  final _searchFocus = FocusNode();
  String _query = '';

  @override
  void initState() {
    super.initState();
    _searchFocus.addListener(() { if (mounted) setState(() {}); });
    // ввод собственной клавиатурой меняет controller программно и onChanged не вызывает — запрос берётся из самого контроллера
    _searchCtrl.addListener(() { if (mounted && _query != _searchCtrl.text) setState(() => _query = _searchCtrl.text); });
    _tabs = TabController(length: 4, vsync: this);
    _tabs.addListener(() {
      if (!mounted) return;
      setState(() {});   // кнопка «+» зависит от вкладки
      if (_tabs.index == 3 && !_tabs.indexIsChanging) _filesKey.currentState?.reload();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      context.read<AppState>().refreshContacts();
      NotificationService.cancelAll();   // события просмотрены — гасим значок у иконки
      _handlePendingNotification();
      _askNickOnce();
    });
  }

  /// Профиль: ник обязателен (его увидят другие пользователи узла и участники групп; другого имени в группах нет), аватар — по желанию
  /// (в настройках). Окно не закрывается, пока ник не введён.
  Future<void> _askNickOnce() async {
    final state = context.read<AppState>();
    if (state.myNick.isNotEmpty || !mounted) return;
    final ctrl = TextEditingController();
    String? error;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => PopScope(
        canPop: false,
        child: StatefulBuilder(builder: (ctx, setS) {
          Future<void> save() async {
            final nick = ctrl.text.trim();
            if (nick.isEmpty) { setS(() => error = 'Введите ник: без него вас не увидят другие'); return; }
            final err = await state.setMyNick(nick);
            if (err != null) { setS(() => error = err); return; }
            if (ctx.mounted) Navigator.pop(ctx);
          }
          return AlertDialog(
            backgroundColor: AppTheme.surface,
            title: const Text('Как вас называть?', style: TextStyle(color: AppTheme.text)),
            content: SingleChildScrollView(
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
                SecureField(controller: ctrl, hint: 'Ник (обязательно: его увидят пользователи узла и участники групп)', onSubmitted: save),
                const SizedBox(height: 8),
                const Text('Картинку профиля можно добавить позже в «Настройках» (не обязательно).', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                if (error != null) Padding(padding: const EdgeInsets.only(top: 8), child: Text(error!, style: const TextStyle(color: AppTheme.incoming))),
              ]),
            ),
            actions: [TextButton(onPressed: save, child: const Text('Сохранить', style: TextStyle(color: AppTheme.accent)))],
          );
        }),
      ),
    );
    ctrl.dispose();
  }

  @override
  void dispose() {
    _tabs.dispose();
    _searchCtrl.dispose();
    _searchFocus.dispose();
    super.dispose();
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

  void _openChat(BuildContext context, Contact c) {
    Navigator.push(
      context,
      MaterialPageRoute(builder: (_) => ChatScreen(peerId: c.peerId, title: c.displayName)),
    );
  }

  /// Долгое нажатие по контакту: открыть чат, очистить чат, удалить контакт.
  /// Своя картинка контакту: из галереи, эмодзи или вернуть ту, что он задал себе. Видна только мне.
  Future<void> _pickContactPicture(BuildContext context, AppState state, Contact c) async {
    const emojis = ['😀', '😎', '🤓', '🥳', '😇', '🤠', '😺', '🐱', '🐶', '🦊', '🐼', '🦁', '🐸', '🦉', '🐧', '🦄'];
    final choice = await showModalBottomSheet<String>(
      context: context,
      backgroundColor: AppTheme.surface,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(leading: const Icon(Icons.photo_library_outlined, color: AppTheme.accent), title: const Text('Из галереи', style: TextStyle(color: AppTheme.text)), onTap: () => Navigator.pop(ctx, 'photo')),
          ListTile(leading: const Icon(Icons.emoji_emotions_outlined, color: AppTheme.accent), title: const Text('Эмодзи', style: TextStyle(color: AppTheme.text)), onTap: () => Navigator.pop(ctx, 'emoji')),
          if (state.hasContactPicture(c.peerId))
            ListTile(leading: const Icon(Icons.undo, color: AppTheme.text), title: const Text('Как он сам', style: TextStyle(color: AppTheme.text)), onTap: () => Navigator.pop(ctx, 'clear')),
        ]),
      ),
    );
    if (choice == null || !context.mounted) return;
    String? err;
    if (choice == 'clear') {
      await state.clearContactPicture(c.peerId);
    } else if (choice == 'photo') {
      final res = await FilePicker.platform.pickFiles(type: FileType.image, withData: true);
      if (res == null) return;
      try {
        final bytes = res.files.single.bytes;
        if (bytes == null) return;
        err = await state.setContactPicture(c.peerId, bytes);
      } finally {
        for (final f in res.files) {
          if (f.path != null) await state.dropPickerCopy(f.path!);
        }
      }
    } else {
      final e = await showDialog<String>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: AppTheme.surface,
          title: const Text('Эмодзи для контакта', style: TextStyle(color: AppTheme.text)),
          content: SizedBox(
            width: 280,
            child: GridView.count(
              crossAxisCount: 4, shrinkWrap: true,
              children: [for (final x in emojis) InkWell(onTap: () => Navigator.pop(ctx, x), child: Center(child: Text(x, style: const TextStyle(fontSize: 28))))],
            ),
          ),
        ),
      );
      if (e == null) return;
      err = await state.setContactPicture(c.peerId, null, emoji: e);
    }
    if (err != null && context.mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
  }

  void _showContactMenu(BuildContext context, AppState state, Contact c) {
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      shape: const RoundedRectangleBorder(borderRadius: BorderRadius.vertical(top: Radius.circular(16))),
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(c.displayName, style: const TextStyle(color: AppTheme.text, fontSize: 17, fontWeight: FontWeight.w600)),
              ),
            ),
            ListTile(
              leading: const Icon(Icons.chat_bubble_outline, color: AppTheme.accent),
              title: const Text('Открыть чат', style: TextStyle(color: AppTheme.text)),
              onTap: () { Navigator.pop(ctx); _openChat(context, c); },
            ),
            if (c.saved)
              ListTile(
                leading: const Icon(Icons.edit_outlined, color: AppTheme.text),
                title: const Text('Переименовать', style: TextStyle(color: AppTheme.text)),
                onTap: () { Navigator.pop(ctx); _renameContact(context, state, c); },
              ),
            if (c.saved)
              ListTile(
                leading: const Icon(Icons.image_outlined, color: AppTheme.text),
                title: const Text('Картинка контакта', style: TextStyle(color: AppTheme.text)),
                subtitle: const Text('Только у вас; ему ничего не отправляется', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                onTap: () { Navigator.pop(ctx); _pickContactPicture(context, state, c); },
              ),
            ListTile(
              leading: const Icon(Icons.cleaning_services_outlined, color: AppTheme.text),
              title: const Text('Очистить чат', style: TextStyle(color: AppTheme.text)),
              subtitle: const Text('Удалится только у вас', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () async {
                Navigator.pop(ctx);
                final ok = await _confirm(context, 'Очистить чат?', 'Переписка с «${c.displayName}» удалится только у вас.', 'Очистить');
                if (ok) await state.clearChat(c.peerId);
              },
            ),
            if (c.peerId != state.nodePeerId)
              ListTile(
                leading: const Icon(Icons.flag_outlined, color: Colors.redAccent),
                title: const Text('Пожаловаться', style: TextStyle(color: AppTheme.text)),
                onTap: () { Navigator.pop(ctx); showComplaintSheet(context, kind: 'user', targetId: c.peerId, title: c.displayName); },
              ),
            ListTile(
              leading: const Icon(Icons.person_remove_outlined, color: Colors.redAccent),
              title: const Text('Удалить контакт', style: TextStyle(color: Colors.redAccent)),
              subtitle: const Text('Перейдёт в «не в контактах»', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () async {
                Navigator.pop(ctx);
                final ok = await _confirm(context, 'Удалить контакт?', '«${c.displayName}» уйдёт из контактов (переписка останется, собеседник появится в разделе «не в контактах»).', 'Удалить');
                if (ok) {
                  await state.removeManualContact(c.peerId);
                  if (context.mounted) {
                    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('«${c.displayName}» перенесён в поиск (не в контактах)')));
                  }
                }
              },
            ),
            ListTile(
              leading: const Icon(Icons.block, color: Colors.redAccent),
              title: const Text('В чёрный список', style: TextStyle(color: Colors.redAccent)),
              subtitle: const Text('Не сможет писать и звонить; вы ему тоже', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              onTap: () async {
                Navigator.pop(ctx);
                final ok = await _confirm(context, 'В чёрный список?', '«${c.displayName}» пропадёт из списка. Блок двусторонний.', 'Заблокировать');
                if (ok) await state.blockContact(c.peerId, c.displayName.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim());
              },
            ),
          ],
        ),
      ),
    );
  }

  /// Переименовать контакт: имя хранится на телефоне и сильнее имени с узла.
  Future<void> _renameContact(BuildContext context, AppState state, Contact c) async {
    final ctrl = TextEditingController(text: c.displayName.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim());
    final name = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: const Text('Переименовать контакт', style: TextStyle(color: AppTheme.text)),
        content: SecureTextField(
          key: const ValueKey('rename-field'),
          controller: ctrl,
          autofocus: true,
          maxLength: 64,
          style: const TextStyle(color: AppTheme.text),
          decoration: const InputDecoration(hintText: 'Имя контакта', hintStyle: TextStyle(color: AppTheme.textSecondary)),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(
            key: const ValueKey('rename-ok'),
            onPressed: () => Navigator.pop(ctx, ctrl.text.trim()),
            child: const Text('Сохранить', style: TextStyle(color: AppTheme.accent)),
          ),
        ],
      ),
    );
    ctrl.dispose();
    if (name == null || name.isEmpty) return;
    await state.addManualContact(c.peerId, name);
  }

  Future<bool> _confirm(BuildContext context, String title, String body, String action) async {
    final r = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: Text(title, style: const TextStyle(color: AppTheme.text)),
        content: Text(body, style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary))),
          TextButton(onPressed: () => Navigator.pop(ctx, true),
              child: Text(action, style: const TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    return r ?? false;
  }

  /// Нажали на того, кто ещё не в контактах: открыть чат или добавить.
  void _showContactSheet(BuildContext context, AppState state, Contact c) {
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.surface,
      shape: const RoundedRectangleBorder(borderRadius: BorderRadius.vertical(top: Radius.circular(16))),
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 16, 20, 4),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(c.displayName, style: const TextStyle(color: AppTheme.text, fontSize: 17, fontWeight: FontWeight.w600)),
                  const SizedBox(height: 2),
                  const Text('Не в ваших контактах', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
                ],
              ),
            ),
            ListTile(
              leading: const Icon(Icons.chat_bubble_outline, color: AppTheme.accent),
              title: const Text('Открыть чат', style: TextStyle(color: AppTheme.text)),
              onTap: () { Navigator.pop(ctx); _openChat(context, c); },
            ),
            ListTile(
              leading: const Icon(Icons.person_add_alt_1, color: AppTheme.accent),
              title: const Text('Добавить в контакты', style: TextStyle(color: AppTheme.text)),
              onTap: () { Navigator.pop(ctx); showSaveContactDialog(context, c.peerId, c.displayName); },
            ),
          ],
        ),
      ),
    );
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
            tooltip: 'Чёрный список',
            icon: const YandiIcon(YandiGlyph.blacklist, color: AppTheme.textSecondary, label: 'Чёрный список'),
            onPressed: () => Navigator.push(context,
                MaterialPageRoute(builder: (_) => const BlacklistScreen())),
          ),
          IconButton(
            tooltip: 'Настройки',
            icon: const YandiIcon(YandiGlyph.settings, color: AppTheme.textSecondary, label: 'Настройки'),
            onPressed: () {
              unawaited(state.refreshProxyInfo());   // статус прокси узла берём заново при открытии настроек
              Navigator.push(context, MaterialPageRoute(builder: (_) => const SettingsScreen()));
            },
          ),
        ],
        bottom: TabBar(
          controller: _tabs,
          labelPadding: EdgeInsets.zero,   // четыре вкладки в ряд: иначе «Контакты» обрезается
          labelStyle: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
          unselectedLabelStyle: const TextStyle(fontSize: 13),
          indicatorColor: AppTheme.accent,
          labelColor: AppTheme.accent,
          unselectedLabelColor: AppTheme.textSecondary,
          tabs: const [
            Tab(icon: YandiIcon(YandiGlyph.contacts, label: 'Контакты')),
            Tab(icon: YandiIcon(YandiGlyph.search, label: 'Поиск')),
            Tab(icon: YandiIcon(YandiGlyph.groups, label: 'Группы')),
            Tab(icon: YandiIcon(YandiGlyph.folder, label: 'Файлы')),
          ],
        ),
      ),
      // на вкладке «Группы» своя кнопка «+ Группа» (иначе две кнопки накладываются друг на друга)
      floatingActionButton: _tabs.index == 2
          ? null
          : _tabs.index == 3
          ? null // файлы сюда вручную не добавляются: только то, что пришло в чатах и группах
          : FloatingActionButton(
              backgroundColor: AppTheme.accent,
              foregroundColor: Colors.black,
              onPressed: () => Navigator.of(context).push(MaterialPageRoute(builder: (_) => const AddContactScreen())),
              child: const Icon(Icons.person_add),
            ),
      body: Column(children: [
        if (state.appUpdate != null) _updateBanner(context, state),
        Expanded(
          child: TabBarView(
            controller: _tabs,
            children: [
              _contactsBody(context, state),
              _searchBody(context, state),
              const GroupsTab(),
              FilesTab(key: _filesKey),
            ],
          ),
        ),
      ]),
    );
  }

  /// Сообщение о новой версии: узел проверил подпись выпуска; приложение ничего не качает само — ссылка на страницу загрузки копируется.
  Widget _updateBanner(BuildContext context, AppState state) {
    final u = state.appUpdate!;
    final page = '${u['page'] ?? ''}';
    return Container(
      key: const ValueKey('app-update-banner'),
      width: double.infinity,
      color: AppTheme.surface,
      padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
      child: Row(children: [
        Expanded(child: Text('Доступна новая версия ${u['version']}. Скачайте её на странице загрузки.',
            style: const TextStyle(color: AppTheme.text, fontSize: 13))),
        TextButton(
          onPressed: () async {
            await Clipboard.setData(ClipboardData(text: page));
            if (context.mounted) {
              ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Ссылка скопирована')));
            }
          },
          child: const Text('Скопировать ссылку'),
        ),
        TextButton(onPressed: state.dismissAppUpdate, child: const Text('Позже')),
      ]),
    );
  }

  Widget _contactsBody(BuildContext context, AppState state) {
    final inner = _contactsBodyInner(context, state);
    final intros = state.introRequests;
    if (intros.isEmpty) return inner;
    return Column(children: [
      IntroRequests(peers: intros, onAccept: (c) => showSaveContactDialog(context, c.peerId, c.displayName)),
      Expanded(child: inner),
    ]);
  }

  Widget _contactsBodyInner(BuildContext context, AppState state) {
    // на вкладке «Контакты» — только добавленные; остальные устройства и люди — на вкладке поиска
    final mine = state.contacts.where((c) => c.saved).toList();
    if (mine.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.people_outline, color: AppTheme.textSecondary, size: 64),
            const SizedBox(height: 16),
            const Text('Нет контактов', style: TextStyle(color: AppTheme.textSecondary)),
            const SizedBox(height: 4),
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 32),
              child: Text('Другие устройства и те, кто писал вам, — на вкладке поиска (лупа).',
                  textAlign: TextAlign.center, style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ),
            const SizedBox(height: 8),
            TextButton.icon(
              onPressed: () => state.refreshContacts(),
              icon: const Icon(Icons.refresh, color: AppTheme.accent),
              label: const Text('Обновить', style: TextStyle(color: AppTheme.accent)),
            ),
          ],
        ),
      );
    }
    return RefreshIndicator(
      color: AppTheme.accent,
      backgroundColor: AppTheme.surface,
      onRefresh: () => state.refreshContacts(),
      child: _ContactList(
        contacts: mine,
        onOpen: (c) => _openChat(context, c),
        onTapUnsaved: (c) => _showContactSheet(context, state, c),
        onDelete: (c) => state.removeManualContact(c.peerId),
        onMenu: (c) => _showContactMenu(context, state, c),
      ),
    );
  }

  Widget _searchBody(BuildContext context, AppState state) {
    final q = _query.trim().toLowerCase();
    String plain(String n) => n.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim().toLowerCase();
    // без запроса — все, кого нет в контактах (с прокруткой); с запросом — поиск по всем
    final found = q.isEmpty
        ? state.contacts.where((c) => !c.saved).toList()
        : state.contacts.where((c) => plain(c.displayName).contains(q) || c.peerId.toLowerCase().contains(q)).toList();
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 6),
          child: TextField(
            controller: _searchCtrl,
            focusNode: _searchFocus,
            onChanged: (v) => setState(() => _query = v),
            style: const TextStyle(color: AppTheme.text),
            keyboardType: SecureKeyboard.inputType,
            enableSuggestions: false,
            autocorrect: false,
            enableIMEPersonalizedLearning: false,
            decoration: InputDecoration(
              hintText: 'Поиск контактов и групп',
              hintStyle: const TextStyle(color: AppTheme.textSecondary),
              prefixIcon: const Icon(Icons.search, color: AppTheme.textSecondary),
              suffixIcon: Row(mainAxisSize: MainAxisSize.min, children: [
                if (_query.isNotEmpty)
                  IconButton(
                    icon: const Icon(Icons.close, color: AppTheme.textSecondary),
                    onPressed: () => setState(() { _query = ''; _searchCtrl.clear(); }),
                  ),
                IconButton(
                  tooltip: 'Сканировать QR',
                  icon: const Icon(Icons.qr_code_scanner, color: AppTheme.accent),
                  onPressed: () async {
                    final raw = await Navigator.of(context).push<String>(MaterialPageRoute(builder: (_) => const QrScanScreen(title: 'QR контакта')));
                    final id = raw == null ? null : extractPeerId(raw);
                    if (!context.mounted || raw == null) return;
                    if (isPairingPayload(raw)) {
                      Navigator.of(context).push(MaterialPageRoute(builder: (_) => PairScreen(initialPayload: raw)));
                      return;
                    }
                    Navigator.of(context).push(MaterialPageRoute(builder: (_) => AddContactScreen(initialId: id ?? '')));
                  },
                ),
                IconButton(
                  tooltip: 'Вставить ID',
                  icon: const Icon(Icons.content_paste, color: AppTheme.accent),
                  onPressed: () async {
                    final data = await Clipboard.getData(Clipboard.kTextPlain);
                    final text = data?.text ?? '';
                    final id = extractPeerId(text);
                    if (!context.mounted) return;
                    if (isPairingPayload(text)) {
                      Navigator.of(context).push(MaterialPageRoute(builder: (_) => PairScreen(initialPayload: text)));
                      return;
                    }
                    if (id == null) {
                      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('В буфере обмена нет ID (64 знака)')));
                      return;
                    }
                    Navigator.of(context).push(MaterialPageRoute(builder: (_) => AddContactScreen(initialId: id)));
                  },
                ),
              ]),
              filled: true, fillColor: AppTheme.surface,
              border: OutlineInputBorder(borderRadius: BorderRadius.circular(10), borderSide: BorderSide.none),
            ),
          ),
        ),
        Expanded(
          child: q.isEmpty
              ? const SearchHub()
              : found.isEmpty
              ? const Center(child: Text('Ничего не найдено', style: TextStyle(color: AppTheme.textSecondary)))
              : RefreshIndicator(
                  color: AppTheme.accent,
                  backgroundColor: AppTheme.surface,
                  onRefresh: () => state.refreshContacts(),
                  child: _ContactList(
                      contacts: found,
                      onOpen: (c) => _openChat(context, c),
                      onTapUnsaved: (c) => _showContactSheet(context, state, c),
                      onDelete: (c) => state.removeManualContact(c.peerId),
                      onMenu: (c) => _showContactMenu(context, state, c),
                    ),
                ),
        ),
        if (_searchFocus.hasFocus) SecureKeyboard(controller: _searchCtrl),
      ],
    );
  }
}

/// Сохранённые контакты сверху, ниже раздел «Другие устройства» (видны, но ещё не в контактах). Устройства, которых нет в сети дольше суток
/// и которых нет в контактах, на главном экране не показываются: они в «Скрытых» и возвращаются сами, когда появятся в сети.
class _ContactList extends StatefulWidget {
  final List<Contact> contacts;
  final void Function(Contact) onOpen;
  final void Function(Contact) onTapUnsaved;
  final void Function(Contact) onDelete;
  final void Function(Contact) onMenu;
  const _ContactList({required this.contacts, required this.onOpen, required this.onTapUnsaved, required this.onDelete, required this.onMenu});

  @override
  State<_ContactList> createState() => _ContactListState();
}

class _ContactListState extends State<_ContactList> {
  bool _showHidden = false;

  List<Contact> get contacts => widget.contacts;
  void Function(Contact) get onOpen => widget.onOpen;
  void Function(Contact) get onTapUnsaved => widget.onTapUnsaved;
  void Function(Contact) get onDelete => widget.onDelete;
  void Function(Contact) get onMenu => widget.onMenu;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final now = DateTime.now().millisecondsSinceEpoch;
    final saved = contacts.where((c) => c.saved).toList();
    final unsaved = contacts.where((c) => !c.saved).toList();
    // устройство со входящими непрочитанными не прячем, чтобы не потерять сообщение
    final hidden = unsaved.where((c) => c.isStale(now) && state.unreadFor(c.peerId) == 0).toList();
    final other = _showHidden ? unsaved : unsaved.where((c) => !hidden.contains(c)).toList();
    final rows = <Widget>[
      for (final c in saved)
        _ContactTile(
          peerId: c.peerId, displayName: c.displayName, online: c.online, isManual: c.isManual, saved: true,
          unread: context.watch<AppState>().unreadFor(c.peerId),
          onTap: () => onOpen(c),
          onLongPress: () => onMenu(c),
          onDelete: c.isManual ? () => onDelete(c) : null,
        ),
      if (other.isNotEmpty)
        const Padding(
          padding: EdgeInsets.fromLTRB(16, 16, 16, 6),
          child: Text('ДРУГИЕ УСТРОЙСТВА, НЕ В КОНТАКТАХ',
              style: TextStyle(color: AppTheme.textSecondary, fontSize: 11, letterSpacing: 1)),
        ),
      for (final c in other)
        _ContactTile(
          peerId: c.peerId, displayName: c.displayName, online: c.online, isManual: c.isManual, saved: false,
          unread: context.watch<AppState>().unreadFor(c.peerId),
          onTap: () => onTapUnsaved(c),
          onAdd: () => showSaveContactDialog(context, c.peerId, c.displayName),
        ),
      if (hidden.isNotEmpty)
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
          child: TextButton.icon(
            onPressed: () => setState(() => _showHidden = !_showHidden),
            icon: const YandiIcon(YandiGlyph.offline, color: AppTheme.textSecondary, size: 18),
            label: Text(_showHidden ? 'Скрыть неактивные устройства' : 'Скрытые устройства (${hidden.length}) — давно не в сети',
                style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
          ),
        ),
    ];
    return ListView(physics: const AlwaysScrollableScrollPhysics(), children: rows);
  }
}

/// Диалог «Добавить в контакты»: имя можно поправить. Используется и в списке, и в чате.
void showSaveContactDialog(BuildContext context, String peerId, String currentName) {
  final ctrl = TextEditingController(text: currentName.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim());
  showDialog(
    context: context,
    builder: (ctx) => AlertDialog(
      backgroundColor: AppTheme.surface,
      title: const Text('Добавить в контакты', style: TextStyle(color: AppTheme.text)),
      content: SingleChildScrollView(child: SecureField(controller: ctrl, hint: 'Имя контакта')),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(ctx),
          child: const Text('Отмена', style: TextStyle(color: AppTheme.textSecondary)),
        ),
        TextButton(
          onPressed: () {
            context.read<AppState>().addManualContact(peerId, ctrl.text);
            Navigator.pop(ctx);
          },
          child: const Text('Добавить', style: TextStyle(color: AppTheme.accent)),
        ),
      ],
    ),
  );
}

class _ContactTile extends StatelessWidget {
  final String      peerId;
  final String      displayName;
  final bool        online;
  final bool        isManual;
  final bool        saved;
  final int         unread;
  final VoidCallback? onAdd;
  final VoidCallback onTap;
  final VoidCallback? onLongPress;
  final VoidCallback? onDelete;

  const _ContactTile({
    required this.peerId,
    required this.displayName,
    required this.online,
    required this.isManual,
    required this.onTap,
    this.saved = true,
    this.unread = 0,
    this.onAdd,
    this.onLongPress,
    this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final tile = InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        decoration: const BoxDecoration(
          border: Border(bottom: BorderSide(color: AppTheme.divider, width: 0.5)),
        ),
        child: Row(
          children: [
            Stack(
              children: [
                AvatarCircle(peerId: peerId, label: displayName, showImage: saved),
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
                      Flexible(child: Text(displayName, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(
                          color: AppTheme.text,
                          fontWeight: FontWeight.w600,
                          fontSize: 15))),
                      if (isManual) ...[
                        const SizedBox(width: 6),
                        const Icon(Icons.person, size: 12, color: AppTheme.accent),
                      ],
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    (online ? 'онлайн' : 'офлайн') + (saved ? '' : ' · не в контактах'),
                    style: TextStyle(
                      color: online ? Colors.greenAccent : AppTheme.textSecondary,
                      fontSize: 12,
                    ),
                  ),
                ],
              ),
            ),
            if (unread > 0) ...[
              Container(
                constraints: const BoxConstraints(minWidth: 22),
                height: 22,
                padding: const EdgeInsets.symmetric(horizontal: 7),
                decoration: const BoxDecoration(color: Colors.redAccent, borderRadius: BorderRadius.all(Radius.circular(11))),
                alignment: Alignment.center,
                child: Text(unread > 99 ? '99+' : '$unread',
                    style: const TextStyle(color: Colors.white, fontSize: 12, fontWeight: FontWeight.bold)),
              ),
              const SizedBox(width: 8),
            ],
            if (onAdd != null)
              IconButton(
                tooltip: 'Добавить в контакты',
                icon: const Icon(Icons.person_add_alt_1, color: AppTheme.accent),
                onPressed: onAdd,
              )
            else
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
