import 'dart:async';
import 'package:flutter/material.dart';
import '../preferences.dart';
import '../theme.dart';
import 'guest_bootstrap.dart';
import 'guest_client.dart';

// Урезанная оболочка гостя: ПК без своего узла входит гостем на ЧУЖОЙ узел (как телефон). Закрытые возможности показываются,
// но по нажатию объясняют, что они откроются после полной авторизации. Звонков здесь пока нет — и об этом сказано честно.

const _yellow = Color(0xFFFFC107);

/// Обёртка для запуска с `--guest`: своё окно приложения с темой оболочки и переключателем языка.
class GuestApp extends StatefulWidget {
  const GuestApp({super.key});
  @override
  State<GuestApp> createState() => _GuestAppState();
}

class _GuestAppState extends State<GuestApp> {
  final prefs = AppPreferences();
  @override
  void dispose() {
    prefs.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => PreferencesScope(
        preferences: prefs,
        child: MaterialApp(title: 'YANDI', debugShowCheckedModeBanner: false, theme: Icq.theme, home: const GuestShell()),
      );
}

/// Жёлтое окно: закрытая гостю возможность не прячется, а объясняется.
Future<void> showLockedNotice(BuildContext context, {String? what, String title = 'Доступно после полной авторизации', String? body}) => showDialog<void>(
      context: context,
      builder: (ctx) => Dialog(
        backgroundColor: Icq.background,
        shape: const RoundedRectangleBorder(side: BorderSide(color: _yellow, width: 2)),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 460),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                const Icon(Icons.lock_outline, color: _yellow),
                const SizedBox(width: 8),
                Expanded(child: UiText(title, style: const TextStyle(color: Color(0xFF8A6500), fontWeight: FontWeight.bold, fontSize: 16))),
              ]),
              const SizedBox(height: 12),
              if (what != null) Padding(padding: const EdgeInsets.only(bottom: 8), child: UiText(what, style: const TextStyle(fontWeight: FontWeight.w600))),
              UiText(body ?? 'Эта возможность открывается, когда администратор узла подтвердит ваш профиль. Запрос можно отправить на вкладке «Профиль».'),
              const SizedBox(height: 12),
              Align(alignment: Alignment.centerRight, child: SolidButton('Понятно', onPressed: () => Navigator.pop(ctx))),
            ]),
          ),
        ),
      ),
    );

class GuestShell extends StatefulWidget {
  /// Подстановки для проверки без сети.
  final GuestBootstrapLoader? loader;
  final Future<GuestClient> Function(GuestNode)? enter;
  const GuestShell({super.key, this.loader, this.enter});
  @override
  State<GuestShell> createState() => _GuestShellState();
}

class _GuestShellState extends State<GuestShell> {
  GuestClient? client;
  List<GuestNode>? nodes; // null — список ещё грузится
  bool fromEmbedded = false, busy = false;
  String? error;
  final code = TextEditingController(), addr = TextEditingController();

  @override
  void initState() {
    super.initState();
    loadNodes();
  }

  @override
  void dispose() {
    code.dispose();
    addr.dispose();
    client?.close();
    super.dispose();
  }

  Future<void> loadNodes() async {
    final r = await (widget.loader ?? GuestBootstrapLoader()).load();
    if (!mounted) return;
    setState(() {
      nodes = r?.doc.guestNodes() ?? const [];
      fromEmbedded = r?.source == BootstrapSource.embedded;
    });
  }

  /// Общий вход: клиент создаётся переданной функцией, затем регистрируются ключи и открывается сокет.
  Future<void> enter(Future<GuestClient> Function() make) async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    GuestClient? c;
    try {
      c = await make();
      await c.start();
      if (!mounted) {
        await c.close();
        return;
      }
      setState(() => client = c);
    } on GuestError catch (e) {
      await c?.close();
      if (mounted) setState(() => error = e.message);
    } catch (_) {
      await c?.close();
      if (mounted) setState(() => error = 'Не удалось войти гостем. Попробуйте ещё раз.');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> enterManual() async {
    final pc = PairCode.parse(code.text);
    if (pc == null) {
      setState(() => error = 'Код сопряжения не распознан. Нужен код вида YANDI-PAIR-1:… с отпечатком сертификата.');
      return;
    }
    String? host;
    int? port;
    final a = addr.text.trim();
    if (a.isNotEmpty) {
      if (!validMobileAddr(a)) {
        setState(() => error = 'Адрес в виде host:port, порт 1–65535.');
        return;
      }
      final i = a.lastIndexOf(':');
      host = a.substring(0, i);
      if (host.startsWith('[') && host.endsWith(']')) host = host.substring(1, host.length - 1);
      port = int.parse(a.substring(i + 1));
    }
    await enter(() => GuestClient.enterWithCode(pc, host: host, port: port));
  }

  Future<void> leave() async {
    final c = client;
    setState(() => client = null);
    await c?.close();
  }

  Widget langToggle() {
    final p = PreferencesScope.of(context);
    return TextButton(
      onPressed: p == null
          ? null
          : () {
              p.english = !p.english;
              p.changed();
            },
      child: Text(p?.english == true ? 'Русский' : 'English', style: const TextStyle(color: Colors.white)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = client;
    return Scaffold(
      body: SafeArea(
        child: Column(children: [
          PageHeader(
            c == null ? 'YANDI · Вход гостем' : 'YANDI · Гость',
            trailing: Row(mainAxisSize: MainAxisSize.min, children: [
              langToggle(),
              if (c != null) SolidButton('Выйти', tone: SolidTone.light, onPressed: leave),
              if (c == null && Navigator.of(context).canPop()) SolidButton('Назад', tone: SolidTone.light, onPressed: () => Navigator.of(context).pop()),
            ]),
          ),
          Expanded(
            child: c == null
                ? SingleChildScrollView(
                    padding: const EdgeInsets.all(12),
                    child: Center(child: ConstrainedBox(constraints: const BoxConstraints(maxWidth: 800), child: entryView())),
                  )
                : GuestSession(key: ValueKey(c), client: c, onLost: leave),
          ),
        ]),
      ),
    );
  }

  Widget entryView() {
    final list = nodes;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      const Panel(
        title: 'Вы подключаетесь к частной сети YANDI',
        child: UiText(
            'Здесь нет вашего узла: вы входите гостем на чужой узел с ограниченными возможностями и можете написать администратору запрос на статус пользователя. Гость живёт на узле сутки, затем стирается; закрытие оболочки — это новый гость.'),
      ),
      const SizedBox(height: 12),
      if (error != null) NoticePanel(error!, error: true),
      if (busy) const Padding(padding: EdgeInsets.only(bottom: 12), child: LinearProgressIndicator()),
      Panel(
        title: 'Узлы, принимающие гостей',
        child: list == null
            ? const Center(child: CircularProgressIndicator())
            : list.isEmpty
                ? const UiText('Сейчас нет узлов, принимающих гостей. Введите код сопряжения ниже или попросите его у администратора узла.')
                : Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                    if (fromEmbedded) const Padding(padding: EdgeInsets.only(bottom: 8), child: UiText('Список узлов взят из копии в программе: сайт недоступен.')),
                    for (final n in list)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 4),
                        child: Row(children: [
                          Expanded(child: Text('${n.name}:${n.port}')),
                          SolidButton('Войти гостем', onPressed: busy ? null : () => enter(() => (widget.enter ?? GuestClient.enterGuest)(n))),
                        ]),
                      ),
                  ]),
      ),
      const SizedBox(height: 16),
      Panel(
        title: 'Вход по коду сопряжения',
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          const UiText('Код YANDI-PAIR-1:… даёт владелец узла. В нём есть отпечаток сертификата: без него вход не выполняется. Адрес можно не вводить, он есть в коде.'),
          const SizedBox(height: 8),
          TextField(controller: code, enabled: !busy, decoration: const InputDecoration(labelText: 'Код сопряжения')),
          const SizedBox(height: 8),
          TextField(controller: addr, enabled: !busy, decoration: const InputDecoration(labelText: 'Адрес host:port (необязательно)')),
          const SizedBox(height: 8),
          Align(alignment: Alignment.centerLeft, child: SolidButton('Войти по коду', onPressed: busy ? null : enterManual)),
        ]),
      ),
    ]);
  }
}

/// Вошедший гость: вкладки «Люди», «Узлы», «Профиль», «Возможности» и чат с одним человеком.
class GuestSession extends StatefulWidget {
  final GuestClient client;
  final VoidCallback onLost;
  const GuestSession({super.key, required this.client, required this.onLost});
  @override
  State<GuestSession> createState() => _GuestSessionState();
}

class _GuestSessionState extends State<GuestSession> {
  static const tabs = ['Люди', 'Узлы', 'Профиль', 'Возможности'];
  String tab = tabs.first;
  GuestUser? chat;
  List<GuestUser>? users;
  List<Map<String, dynamic>>? nodes;
  bool connected = true;
  String statusState = 'none';
  String? notice;
  final messages = <String, List<GuestMessage>>{};
  final unread = <String>{};
  final readSent = <String>{}; // входящие, на которые «прочитано» уже отправлено
  final subs = <StreamSubscription<dynamic>>[];
  Timer? poll;
  final input = TextEditingController(), nick = TextEditingController(), statusText = TextEditingController();

  GuestClient get c => widget.client;

  @override
  void initState() {
    super.initState();
    subs.addAll([
      c.messages.listen(onMessage),
      c.receipts.listen(onReceipt),
      c.connection.listen((v) => mounted ? setState(() => connected = v) : null),
      c.securityErrors.listen((e) => mounted ? setState(() => notice = e) : null),
    ]);
    connected = c.connected;
    refresh();
    poll = Timer.periodic(const Duration(seconds: 15), (_) => refresh());
  }

  @override
  void dispose() {
    poll?.cancel();
    for (final s in subs) {
      s.cancel();
    }
    input.dispose();
    nick.dispose();
    statusText.dispose();
    super.dispose();
  }

  Future<void> refresh() async {
    final u = await c.getUsers();
    final n = tab == 'Узлы' ? await c.getNodes() : null;
    final s = c.isGuest ? await c.getStatusRequest() : 'none';
    if (s == 'accepted') await c.getInfo(); // роль на узле могла смениться: лимит гостя тогда снимается
    if (!mounted) return;
    setState(() {
      users = u ?? users;
      nodes = n ?? nodes;
      statusState = s;
    });
  }

  void onMessage(GuestMessage m) {
    if (!mounted) return;
    setState(() {
      messages.putIfAbsent(m.peerId, () => []).add(m);
      if (chat?.peerId != m.peerId) unread.add(m.peerId);
    });
    if (chat?.peerId == m.peerId) markRead(m.peerId);
  }

  void onReceipt((String, List<String>) r) {
    if (!mounted) return;
    final rank = r.$1 == 'read' ? 2 : 1;
    int rk(String s) => switch (s) { 'read' => 2, 'delivered' => 1, _ => 0 };
    setState(() {
      for (final l in messages.values) {
        for (final m in l) {
          if (m.outgoing && r.$2.contains(m.id) && rk(m.status) < rank) m.status = r.$1;
        }
      }
    });
  }

  /// Сообщить собеседнику «прочитано» по ещё не отмеченным входящим этого чата.
  void markRead(String peer) {
    final ids = [for (final m in messages[peer] ?? const <GuestMessage>[]) if (!m.outgoing && readSent.add(m.id)) m.id];
    if (ids.isNotEmpty) unawaited(c.sendReceipt(peer, 'read', ids));
  }

  void openChat(GuestUser u) {
    setState(() {
      chat = u;
      unread.remove(u.peerId);
      notice = null;
    });
    markRead(u.peerId);
  }

  Future<void> send() async {
    final u = chat;
    final text = input.text.trim();
    if (u == null || text.isEmpty) return;
    if (c.messagesLeft == 0) {
      setState(() => notice = limitText);
      return;
    }
    final m = await c.sendText(u.peerId, text);
    if (!mounted) return;
    setState(() {
      if (m == null) {
        notice = 'Сообщение не отправлено: нет связи с узлом или нет ключа собеседника.';
      } else {
        notice = null;
        input.clear();
        messages.putIfAbsent(u.peerId, () => []).add(m);
      }
    });
  }

  static const limitText = 'Гостю доступно $guestMessagesPerDay сообщений в сутки. Лимит исчерпан — попробуйте позже или запросите статус пользователя.';

  Future<void> complain(GuestUser u) async {
    final text = TextEditingController();
    var reason = 'spam';
    String? error;
    var sent = false, busy = false;
    const reasons = {'spam': 'Спам или навязчивость', 'abuse': 'Оскорбления, травля', 'illegal': 'Запрещённое содержимое', 'danger': 'Угрозы, опасность', 'other': 'Другое'};
    await showDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setS) => AlertDialog(
          title: const UiText('Пожаловаться'),
          content: SizedBox(
            width: 420,
            child: SingleChildScrollView(
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(u.name, style: const TextStyle(fontWeight: FontWeight.bold)),
                const UiText('Жалобу увидит только владелец узла. Тот, на кого жалоба, не узнает, что это вы.'),
                const SizedBox(height: 8),
                for (final e in reasons.entries)
                  InkWell(
                    onTap: busy || sent ? null : () => setS(() => reason = e.key),
                    child: Row(children: [
                      Icon(reason == e.key ? Icons.radio_button_checked : Icons.radio_button_off, size: 20, color: Icq.header),
                      const SizedBox(width: 8),
                      UiText(e.value),
                    ]),
                  ),
                TextField(controller: text, maxLength: 1000, maxLines: 3, decoration: const InputDecoration(labelText: 'Что случилось')),
                if (error != null) UiText(error!, style: const TextStyle(color: Icq.danger)),
                if (sent) const UiText('Жалоба отправлена владельцу узла.'),
              ]),
            ),
          ),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx), child: UiText(sent ? 'Закрыть' : 'Отмена')),
            if (!sent)
              TextButton(
                onPressed: busy
                    ? null
                    : () async {
                        setS(() {
                          busy = true;
                          error = null;
                        });
                        final r = await c.submitComplaint(targetId: u.peerId, reason: reason, text: text.text.trim());
                        setS(() {
                          busy = false;
                          error = r;
                          sent = r == null;
                        });
                      },
                child: const UiText('Отправить'),
              ),
          ],
        ),
      ),
    );
    text.dispose();
  }

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
        padding: const EdgeInsets.all(12),
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 900),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              if (!connected) const NoticePanel('Нет связи с узлом, переподключаюсь…'),
              if (chat != null) chatView(chat!) else ...[
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: TabStrip(
                    labels: tabs,
                    selected: tab,
                    onSelected: (v) {
                      setState(() {
                        tab = v;
                        notice = null;
                      });
                      refresh();
                    },
                  ),
                ),
                if (notice != null) NoticePanel(notice!),
                switch (tab) { 'Узлы' => nodesView(), 'Профиль' => profileView(), 'Возможности' => lockedView(), _ => peopleView() },
              ],
            ]),
          ),
        ),
      );

  Widget peopleView() {
    final list = users;
    return Panel(
      title: 'Люди узла',
      trailing: Text(c.nodeName, style: const TextStyle(color: Colors.white)),
      child: list == null
          ? const UiText('Узел пока не ответил.')
          : list.isEmpty
              ? const UiText('Пока никого нет.')
              : Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                  for (final u in list)
                    // собственную строку гостя в списке не показываем: писать самому себе нельзя
                    if (u.peerId != c.deviceId)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 4),
                        child: Row(children: [
                          Icon(Icons.circle, size: 10, color: u.online ? Icq.online : Icq.dark),
                          const SizedBox(width: 8),
                          Expanded(child: u.isNode ? Row(children: [const UiText('💻 Компьютер'), Text('  ${u.name}')]) : Text(u.name)),
                          if (!u.isNode && u.role == 'guest') const UiText('гость'),
                          if (unread.contains(u.peerId)) const Padding(padding: EdgeInsets.only(left: 8), child: Icon(Icons.mark_chat_unread, size: 18, color: Icq.peer)),
                          const SizedBox(width: 8),
                          SolidButton('Написать', onPressed: () => openChat(u)),
                          if (!u.isNode) ...[const SizedBox(width: 6), SolidButton('Пожаловаться', tone: SolidTone.danger, onPressed: () => complain(u))],
                        ]),
                      ),
                ]),
    );
  }

  Widget chatView(GuestUser u) {
    final list = messages[u.peerId] ?? const <GuestMessage>[];
    final left = c.messagesLeft;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Panel(
        title: u.isNode ? '💻 Компьютер' : 'Чат',
        trailing: SolidButton('К списку', tone: SolidTone.light, onPressed: () => setState(() => chat = null)),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Text(u.name, style: const TextStyle(fontWeight: FontWeight.bold)),
          const SizedBox(height: 8),
          ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 120, maxHeight: 420),
            child: list.isEmpty
                ? const Center(child: UiText('Сообщений пока нет.'))
                : ListView(shrinkWrap: true, reverse: true, children: [
                    for (final m in list.reversed)
                      Align(
                        alignment: m.outgoing ? Alignment.centerRight : Alignment.centerLeft,
                        child: Container(
                          margin: const EdgeInsets.symmetric(vertical: 3),
                          padding: const EdgeInsets.all(8),
                          constraints: const BoxConstraints(maxWidth: 560),
                          color: m.outgoing ? Icq.sent : Icq.light,
                          child: Column(crossAxisAlignment: CrossAxisAlignment.end, children: [
                            SelectableText(m.text),
                            Text('${m.time.hour.toString().padLeft(2, '0')}:${m.time.minute.toString().padLeft(2, '0')}${m.outgoing ? '  ${m.status == 'read' ? '✓✓' : m.status == 'delivered' ? '✓' : '…'}' : ''}',
                                style: const TextStyle(fontSize: 11, color: Icq.dark)),
                          ]),
                        ),
                      ),
                  ]),
          ),
        ]),
      ),
      const SizedBox(height: 12),
      if (notice != null) NoticePanel(notice!, error: true),
      if (left == 0)
        const NoticePanel(limitText)
      else ...[
        TextField(controller: input, maxLength: 2000, minLines: 1, maxLines: 4, decoration: const InputDecoration(labelText: 'Сообщение (только текст)'), onSubmitted: (_) => send()),
        Row(children: [
          SolidButton('Отправить', onPressed: send),
          const SizedBox(width: 12),
          if (left >= 0) ...[const UiText('Осталось сообщений за сутки'), Text(': $left')],
        ]),
      ],
    ]);
  }

  Widget nodesView() {
    final list = nodes;
    return Panel(
      title: 'Узлы',
      child: list == null
          ? const UiText('Узел пока не ответил.')
          : list.isEmpty
              ? const UiText('Узлов в сети не видно')
              : Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                  for (final n in list)
                    Padding(
                      padding: const EdgeInsets.symmetric(vertical: 3),
                      child: Row(children: [
                        Icon(Icons.circle, size: 10, color: n['online'] == true ? Icq.online : Icq.dark),
                        const SizedBox(width: 8),
                        Text('${n['short_id'] ?? ''}  ·  ${n['region'] ?? ''}  ·  ${n['role'] ?? ''}'),
                      ]),
                    ),
                ]),
    );
  }

  Widget profileView() => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Panel(
          title: 'Ник',
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            const UiText('Ник увидят другие люди узла в списке.'),
            TextField(controller: nick, maxLength: 32, decoration: const InputDecoration(labelText: 'Ник')),
            Align(
              alignment: Alignment.centerLeft,
              child: SolidButton('Сохранить', onPressed: () async {
                final t = nick.text.trim();
                if (t.isEmpty) return;
                final ok = await c.putNick(t);
                if (mounted) setState(() => notice = ok ? 'Ник сохранён.' : 'Не удалось сохранить ник.');
                if (ok) unawaited(refresh());
              }),
            ),
          ]),
        ),
        const SizedBox(height: 16),
        Panel(
          title: 'Запрос статуса пользователя',
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            const UiText('Администратор узла решает на своё усмотрение. В обращении не должно быть ссылок.'),
            if (statusState == 'pending') const UiText('Запрос отправлен и ждёт решения.'),
            if (statusState == 'accepted') const UiText('Администратор принял запрос: статус обновлён.'),
            if (statusState == 'rejected') const UiText('Администратор отклонил запрос.'),
            TextField(controller: statusText, maxLength: 300, maxLines: 3, decoration: const InputDecoration(labelText: 'Текст запроса')),
            Align(
              alignment: Alignment.centerLeft,
              child: SolidButton('Отправить запрос', onPressed: statusState == 'pending'
                  ? null
                  : () async {
                      final t = statusText.text.trim();
                      if (t.isEmpty) return;
                      final r = await c.requestUserStatus(t);
                      if (!mounted) return;
                      setState(() => notice = r ?? 'Запрос отправлен администратору узла.');
                      if (r == null) {
                        statusText.clear();
                        unawaited(refresh());
                      }
                    }),
            ),
          ]),
        ),
      ]);

  Widget lockedView() {
    Widget row(String label, {String? body, String? title}) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: InkWell(
            onTap: () => showLockedNotice(context, what: label, body: body, title: title ?? 'Доступно после полной авторизации'),
            child: Padding(
              padding: const EdgeInsets.all(6),
              child: Row(children: [const Icon(Icons.lock_outline, size: 18, color: _yellow), const SizedBox(width: 8), Expanded(child: UiText(label))]),
            ),
          ),
        );
    return Panel(
      title: 'Возможности',
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        const UiText('Эти возможности откроются после подтверждения профиля администратором узла.'),
        const SizedBox(height: 8),
        row('Группы'),
        row('Выход в интернет через узел (VPN)'),
        row('Видеозвонки'),
        row('Приглашать другие устройства'),
        row('Файлы'),
        row('Звонки', title: 'Звонки', body: 'Звонки гостя с ПК появятся в следующей версии'),
      ]),
    );
  }
}
