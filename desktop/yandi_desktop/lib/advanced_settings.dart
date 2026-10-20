import 'dart:async';
import 'theme.dart';
import 'package:flutter/material.dart';
import 'transport/api.dart';
import 'domain/states.dart';
import 'domain/shell_repository.dart';
import 'preferences.dart';
import 'groups.dart';

/// Разбор одной строки прокси так же, как узел: `хост:порт[:логин:пароль]`, `логин:пароль@хост:порт`, IPv6 в скобках (`[::1]:1080`),
/// допустимая схема `socks5://` или `http://` отбрасывается (вид задаёт список выбора). Пароль может содержать двоеточия.
class ProxyLine {
  final String host;
  final int port;
  final String user, password;
  /// Вид из схемы строки (`socks5` / `http_connect`); null — схемы не было, действует выбор в форме.
  final String? kind;
  const ProxyLine(this.host, this.port, this.user, this.password, [this.kind]);

  static (String, int)? _hostPort(String hp) {
    hp = hp.trim();
    String host;
    String portText;
    if (hp.startsWith('[')) {
      final end = hp.indexOf(']');
      if (end < 0 || end + 1 >= hp.length || hp[end + 1] != ':') return null;
      host = hp.substring(1, end);
      portText = hp.substring(end + 2);
    } else {
      final at = hp.lastIndexOf(':');
      if (at < 0) return null;
      host = hp.substring(0, at);
      portText = hp.substring(at + 1);
    }
    final port = int.tryParse(portText);
    if (host.trim().isEmpty || port == null || port < 1 || port > 65535) return null;
    return (host.trim(), port);
  }

  static ProxyLine? parse(String raw) {
    var line = raw.trim();
    String? kind;
    final scheme = RegExp(r'^([A-Za-z][A-Za-z0-9+.-]*)://').firstMatch(line);
    if (scheme != null) {
      // так же строго, как узел: https:// (TLS до прокси) не поддержан — молча понизить до обычного HTTP нельзя (логин и пароль ушли бы открытым текстом)
      kind = switch (scheme.group(1)!.toLowerCase()) {
        'socks5' || 'socks5h' || 'socks' => 'socks5',
        'http' => 'http_connect',
        _ => null,
      };
      if (kind == null) return null;
    }
    line = line.replaceFirst(RegExp(r'^[A-Za-z][A-Za-z0-9+.-]*://'), '').replaceFirst(RegExp(r'/+$'), '');
    if (line.isEmpty) return null;
    final p = _parseBody(line);
    return p == null ? null : ProxyLine(p.host, p.port, p.user, p.password, kind);
  }

  static ProxyLine? _parseBody(String line) {
    final at = line.lastIndexOf('@');
    if (at >= 0) {
      final cred = line.substring(0, at);
      final colon = cred.indexOf(':');
      if (colon <= 0) return null;
      final hp = _hostPort(line.substring(at + 1));
      if (hp == null) return null;
      return ProxyLine(hp.$1, hp.$2, cred.substring(0, colon), cred.substring(colon + 1));
    }
    if (line.startsWith('[')) {
      final end = line.indexOf(']');
      if (end < 0 || end + 1 >= line.length || line[end + 1] != ':') return null;
      final tail = line.substring(end + 2).split(':');
      final port = int.tryParse(tail[0]);
      if (port == null || port < 1 || port > 65535) return null;
      final host = line.substring(1, end);
      if (tail.length == 1) return ProxyLine(host, port, '', '');
      if (tail.length < 3 || tail[1].isEmpty) return null;
      return ProxyLine(host, port, tail[1], tail.sublist(2).join(':'));
    }
    final parts = line.split(':');
    if (parts.length != 2 && parts.length < 4) return null;
    final port = int.tryParse(parts[1]);
    if (parts[0].trim().isEmpty || port == null || port < 1 || port > 65535) return null;
    if (parts.length == 2) return ProxyLine(parts[0].trim(), port, '', '');
    if (parts[2].isEmpty) return null;
    return ProxyLine(parts[0].trim(), port, parts[2], parts.sublist(3).join(':'));
  }

  /// Строка для формы правки: адрес IPv6 — в скобках, иначе её нельзя будет разобрать обратно.
  static String format(String host, int port, {String user = '', String password = ''}) {
    final h = host.contains(':') ? '[$host]' : host;
    if (user.isEmpty) return '$h:$port';
    // «@» в логине или пароле: форма `логин:пароль@адрес` разбирается по последнему «@», а `адрес:порт:логин:пароль` — нет
    if ('$user:$password'.contains('@')) return '$user:$password@$h:$port';
    return '$h:$port:$user:$password';
  }

  /// Подходит ли строка для отправки: тот же разбор, что и при сохранении правки, поэтому форма не пропускает то, что потом отвергнет.
  static bool plausible(String line) => parse(line) != null;
}

class AdvancedSettings extends StatefulWidget {
  final Api api;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const AdvancedSettings({
    super.key,
    required this.api,
    required this.busy,
    required this.isActive,
    required this.mutate,
  });
  @override
  State<AdvancedSettings> createState() => _AdvancedSettingsState();
}

class _AdvancedSettingsState extends State<AdvancedSettings> {
  final cap = TextEditingController();
  Map<String, dynamic>? pool;
  String? error;
  bool loading = false;
  bool poolEnabled = false, poolStrict = false;
  final selected = <String>{};
  final shown = <String>{};
  final revealed = <String, ({String user, String password})>{};
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    refresh();
  }

  @override
  void dispose() {
    pool = null;
    revealed.clear();
    shown.clear();
    cap.clear();
    cap.dispose();
    super.dispose();
  }

  List<Map<String, dynamic>> get proxies =>
      pool == null ? const [] : ShellRepository.list(pool!, 'proxies');

  Future<void> refresh() async {
    if (!active || loading) return;
    loading = true;
    try {
      final data = await widget.api.request('/api/upstream-proxies');
      if (!active) return;
      final ids = ShellRepository.list(data, 'proxies').map((p) => '${p['id']}').toSet();
      setState(() {
        pool = data;
        poolEnabled = data['enabled'] == true;
        poolStrict = data['strict'] == true;
        cap.text = '${data['max_clients']}';
        selected.retainAll(ids);
        shown.retainAll(ids);
        revealed.clear();
        error = null;
      });
    } catch (e) {
      if (active)
        setState(() {
          pool = null;
          error = e is ApiFailure
              ? e.message
              : const ApiFailure('protocol').message;
        });
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  Future<void> sealedAction(Future<void> Function() action) =>
      widget.mutate(() async {
        final v = VaultState.fromJson(
          await widget.api.request('/api/vault/status'),
        );
        if (!active) return;
        if (!v.configured || !v.unlocked || v.hasError || v.migrationPending)
          throw const ApiFailure('sealed_required');
        await action();
        if (active) await refresh();
      });

  String idUrl(Map<String, dynamic> p, [String tail = '']) =>
      '/api/upstream-proxies/${ShellRepository(widget.api).segment('${p['id']}')}$tail';

  /// Проверка до сохранения: узел соединяется через каждую строку и ничего не запоминает.
  Future<List<String>> ping(String text, String kind) async {
    final r = await widget.api.request(
      '/api/upstream-proxies/test',
      body: {'text': text, 'kind': kind},
    );
    return [
      for (final x in ShellRepository.list(r, 'results'))
        x['ok'] == true
            ? '${x['proxy']} — работает, ${x['ms']} мс'
            : '${x['proxy']} — не отвечает: ${x['error'] ?? ''}',
      for (final e in (r['errors'] is List ? r['errors'] as List : const []))
        '$e',
    ];
  }

  Future<({String user, String password})?> reveal(Map<String, dynamic> p) async {
    final id = '${p['id']}';
    final have = revealed[id];
    if (have != null) return have;
    try {
      final r = await widget.api.request(idUrl(p, '/reveal'), body: const {});
      if (!active) return null;
      final v = (user: '${r['user'] ?? ''}', password: '${r['password'] ?? ''}');
      revealed[id] = v;
      return v;
    } catch (e) {
      if (active) {
        setState(
          () => error = e is ApiFailure
              ? e.message
              : const ApiFailure('protocol').message,
        );
      }
      return null;
    }
  }

  Future<void> toggleEye(Map<String, dynamic> p) async {
    final id = '${p['id']}';
    if (shown.contains(id)) {
      setState(() {
        shown.remove(id);
        revealed.remove(id);
      });
      return;
    }
    if (await reveal(p) != null && active) setState(() => shown.add(id));
  }

  Future<void> createProxy() async {
    final body = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => ProxyForm(onPing: ping),
    );
    if (!active || body == null) return;
    try {
      await sealedAction(() async {
        final r = await widget.api.request('/api/upstream-proxies', body: body);
        final errors = r['errors'];
        if (errors is List && errors.isNotEmpty && active) {
          setState(() => error = errors.join('\n'));
        }
      });
    } finally {
      body.clear();
    }
  }

  Future<void> editSelected() async {
    final p = proxies.where((p) => selected.contains('${p['id']}')).toList();
    if (p.length != 1) return;
    final one = p.single;
    final secret = one['auth'] == 'password' ? await reveal(one) : (user: '', password: '');
    if (!active || secret == null) return;
    final line = ProxyLine.format(
      '${one['host']}',
      (one['port'] as num).toInt(),
      user: one['auth'] == 'password' ? secret.user : '',
      password: one['auth'] == 'password' ? secret.password : '',
    );
    final body = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => ProxyForm(
        onPing: ping,
        editing: true,
        initialLine: line,
        initialKind: '${one['kind']}',
        initialLimit: '${one['max_clients'] ?? 0}',
        initialEnabled: one['enabled'] == true,
      ),
    );
    if (!active || body == null) return;
    final limit = int.tryParse('${body['max_clients']}');
    if (limit == null || limit < 0 || limit > 1000) {
      setState(() => error = 'Лимит: 0–1000.');
      return;
    }
    final l = ProxyLine.parse('${body['text']}');
    if (l == null) {
      setState(() => error = 'Формат: ip:port:login:pass (логин и пароль можно не указывать).');
      return;
    }
    try {
      await sealedAction(() async {
        await widget.api.request(
          idUrl(one),
          body: {
            'kind': l.kind ?? body['kind'],
            'host': l.host,
            'port': l.port,
            'auth': l.user.isEmpty ? 'none' : 'password',
            'user': l.user,
            'password': l.password,
            'enabled': body['enabled'],
            'max_clients': limit,
          },
        );
      });
    } finally {
      body.clear();
      revealed.clear();
      shown.clear();
    }
  }

  Future<void> deleteSelected() async {
    final ids = proxies.where((p) => selected.contains('${p['id']}')).toList();
    if (ids.isEmpty) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => GroupConfirm(
        title: ids.length == 1 ? 'Удалить внешний прокси?' : 'Удалить выбранные прокси?',
      ),
    );
    if (!active || ok != true) return;
    await sealedAction(() async {
      for (final p in ids) {
        await widget.api.request(idUrl(p), method: 'DELETE');
      }
      selected.clear();
    });
  }

  Future<void> savePoolSettings() async {
    final n = int.tryParse(cap.text);
    if (n == null || n < 1 || n > 1000) {
      setState(() => error = 'Лимит: 1–1000.');
      return;
    }
    await sealedAction(() async {
      await widget.api.request(
        '/api/upstream-proxies/settings',
        body: {'enabled': poolEnabled, 'strict': poolStrict, 'max_clients': n},
      );
    });
  }

  Widget button(String text, Future<void> Function() action) => TextButton(
    onPressed: widget.busy || loading ? null : action,
    child: UiText(text),
  );

  static const _data = TextStyle(fontSize: 14, fontStyle: FontStyle.italic);
  static String _kind(dynamic k) => k == 'http_connect' ? 'HTTP' : 'SOCKS5';

  Widget proxyRow(Map<String, dynamic> p) {
    final id = '${p['id']}';
    final hasAuth = p['auth'] == 'password';
    final open = shown.contains(id);
    final secret = revealed[id];
    const mask = '••••••';
    final user = open ? (secret?.user ?? '') : mask;
    final pass = open ? (secret?.password ?? '') : mask;
    final address = '${p['host']}:${p['port']}${hasAuth ? ':$user:$pass' : ''}';
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        children: [
          PanelCheck(
            value: selected.contains(id),
            label: null,
            onChanged: widget.busy
                ? null
                : (v) => setState(() => v ? selected.add(id) : selected.remove(id)),
          ),
          const SizedBox(width: 6),
          const SizedBox(
            width: 58,
            child: UiText('PROXY', style: TextStyle(fontSize: 13, fontWeight: FontWeight.bold, color: Icq.dark)),
          ),
          Container(
            width: 64,
            padding: const EdgeInsets.symmetric(vertical: 4),
            alignment: Alignment.center,
            color: p['enabled'] == true ? Icq.header : Icq.dark,
            child: Text(_kind(p['kind']), style: const TextStyle(fontSize: 12, color: Colors.white, fontWeight: FontWeight.bold)),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Container(
              height: 40,
              padding: const EdgeInsets.only(left: 12),
              decoration: BoxDecoration(color: Colors.white, border: Border.all(color: Icq.border)),
              child: Row(
                children: [
                  Flexible(
                    flex: 2,
                    child: Text(
                      '${stateLabel(context, p['state'])} · ${p['clients']} / ${p['cap']}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 11, color: Icq.dark),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    flex: 5,
                    child: Align(
                      alignment: Alignment.centerRight,
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        alignment: Alignment.centerRight,
                        child: Text(address, maxLines: 1, style: _data),
                      ),
                    ),
                  ),
                  if (hasAuth)
                    IconButton(
                      tooltip: tr(context, open ? 'Скрыть логин и пароль' : 'Показать логин и пароль'),
                      onPressed: widget.busy ? null : () => toggleEye(p),
                      icon: Icon(open ? Icons.visibility_off : Icons.visibility, size: 20, color: Icq.header),
                    )
                  else
                    const SizedBox(width: 12),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final list = proxies;
    final busy = widget.busy || loading;
    return PanelStack(
      children: [
        if (loading) const LinearProgressIndicator(),
        if (error != null) UiText(error!),
        button('Обновить настройки', refresh),
        const UiText(
          'Внешние прокси для выхода в интернет',
          style: TextStyle(fontSize: 22),
        ),
        const UiText(
          'Изменения пула требуют открытой настроенной закрытой среды. Пароли хранит только демон в запечатанном файле.',
        ),
        if (pool != null) ...[
          const SizedBox(height: 4),
          if (list.isEmpty)
            const UiText('Прокси ещё не добавлены.')
          else ...[
            PanelCheck(
              value: selected.length == list.length,
              label: 'Выбрать все',
              onChanged: widget.busy
                  ? null
                  : (v) => setState(() {
                      selected.clear();
                      if (v) selected.addAll(list.map((p) => '${p['id']}'));
                    }),
            ),
            const SizedBox(height: 6),
            for (final p in list) proxyRow(p),
          ],
          const SizedBox(height: 8),
          PanelCheck(
            value: poolEnabled,
            label: 'Включить пул',
            onChanged: widget.busy ? null : (v) => setState(() => poolEnabled = v),
          ),
          PanelCheck(
            value: poolStrict,
            label: 'Строго соблюдать лимит клиентов',
            onChanged: widget.busy ? null : (v) => setState(() => poolStrict = v),
          ),
          TextField(
            controller: cap,
            enabled: !widget.busy,
            decoration: InputDecoration(
              labelText: tr(context, 'Общий лимит клиентов'),
            ),
          ),
        ],
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            SolidButton('Добавить', onPressed: busy ? null : createProxy),
            SolidButton('Изменить', onPressed: busy || selected.length != 1 ? null : editSelected),
            SolidButton(
              'Удалить',
              tone: SolidTone.danger,
              onPressed: busy || selected.isEmpty ? null : deleteSelected,
            ),
            SolidButton('Сохранить настройки прокси', onPressed: busy || pool == null ? null : savePoolSettings),
          ],
        ),
      ],
    );
  }
}

class ProxyForm extends StatefulWidget {
  /// Проверка до сохранения: строки → отчёт по каждой.
  final Future<List<String>> Function(String text, String kind)? onPing;
  final bool editing;
  final String initialLine, initialKind, initialLimit;
  final bool initialEnabled;
  const ProxyForm({
    super.key,
    this.onPing,
    this.editing = false,
    this.initialLine = '',
    this.initialKind = 'socks5',
    this.initialLimit = '0',
    this.initialEnabled = true,
  });
  @override
  State<ProxyForm> createState() => _ProxyFormState();
}

class _ProxyFormState extends State<ProxyForm> {
  late final line = TextEditingController(text: widget.initialLine);
  late final limit = TextEditingController(text: widget.initialLimit);
  late String kind = widget.initialKind;
  late bool enabled = widget.initialEnabled;
  String? error;
  List<String> report = const [];
  bool pinging = false;
  @override
  void dispose() {
    line.clear();
    line.dispose();
    limit.dispose();
    super.dispose();
  }

  /// Строки списка; при правке — ровно одна.
  List<String>? lines() {
    final all = line.text.split('\n').map((l) => l.trim()).where((l) => l.isNotEmpty).toList();
    if (all.isEmpty || (widget.editing && all.length != 1) || !all.every(ProxyLine.plausible)) {
      setState(() => error = 'Формат: ip:port:login:pass (логин и пароль можно не указывать), по одному прокси в строке.');
      return null;
    }
    return all;
  }

  Future<void> doPing() async {
    final all = lines();
    if (all == null || widget.onPing == null) return;
    setState(() {
      error = null;
      pinging = true;
      report = const [];
    });
    try {
      final r = await widget.onPing!(all.join('\n'), kind);
      if (mounted) setState(() => report = r);
    } catch (e) {
      if (mounted) {
        setState(
          () => error = e is ApiFailure ? e.message : const ApiFailure('protocol').message,
        );
      }
    } finally {
      if (mounted) setState(() => pinging = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: UiText(widget.editing ? 'Изменить внешний прокси' : 'Добавить внешний прокси'),
    content: SizedBox(
      width: 460,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Align(
              alignment: Alignment.centerLeft,
              child: DropdownButton<String>(
                value: kind,
                items: const [
                  DropdownMenuItem(value: 'socks5', child: Text('SOCKS5')),
                  DropdownMenuItem(value: 'http_connect', child: Text('HTTP')),
                ],
                onChanged: (v) => setState(() => kind = v!),
              ),
            ),
            const SizedBox(height: 16),
            TextField(
              controller: line,
              minLines: 1,
              maxLines: widget.editing ? 1 : 6,
              autocorrect: false,
              enableSuggestions: false,
              style: const TextStyle(fontSize: 14, fontStyle: FontStyle.italic),
              decoration: InputDecoration(
                labelText: tr(context, 'ip:port:login:pass'),
                contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 14),
                border: const OutlineInputBorder(),
                hintText: '203.0.113.5:1080:login:pass',
              ),
            ),
            if (widget.editing) ...[
              const SizedBox(height: 20),
              TextField(
                controller: limit,
                decoration: InputDecoration(
                  labelText: tr(context, 'Клиентов (0 — общий предел)'),
                  contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 14),
                  border: const OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 14),
              PanelCheck(
                value: enabled,
                label: 'Включён',
                onChanged: (v) => setState(() => enabled = v),
              ),
            ],
            if (error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: UiText(error!),
              ),
            for (final r in report)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: UiText(r, style: const TextStyle(fontSize: 13, fontStyle: FontStyle.italic)),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      if (widget.onPing != null)
        SolidButton('Пинг', tone: SolidTone.light, onPressed: pinging ? null : doPing),
      SolidButton(
        widget.editing ? 'Сохранить' : 'Добавить',
        onPressed: () {
          final all = lines();
          if (all == null) return;
          final result = <String, dynamic>{
            'text': all.join('\n'),
            'kind': kind,
            if (widget.editing) ...{
              'max_clients': limit.text.trim(),
              'enabled': enabled,
            },
          };
          line.clear();
          Navigator.pop(context, result);
        },
      ),
    ],
  );
}

/// Опция узла «разрешить сопряжение без QR»: гости (docs/GUEST.md). По умолчанию выключена.
class GuestPairingPanel extends StatefulWidget {
  final Api api;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const GuestPairingPanel({
    super.key,
    required this.api,
    required this.busy,
    required this.isActive,
    required this.mutate,
  });
  @override
  State<GuestPairingPanel> createState() => _GuestPairingPanelState();
}

class _GuestPairingPanelState extends State<GuestPairingPanel> {
  Map<String, dynamic>? data;
  String? error;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    load();
  }

  Timer? retry;
  int attempts = 0;
  @override
  void dispose() {
    retry?.cancel();
    super.dispose();
  }

  /// Результат принимается, пока панель на экране; при сбое загрузка повторяется (иначе переключатель оставался бы выключенным навсегда).
  Future<void> load() async {
    retry?.cancel();
    try {
      final d = await widget.api.request('/api/guest-pairing');
      if (mounted) setState(() { data = d; error = null; attempts = 0; });
    } catch (e) {
      if (!mounted) return;
      setState(() => error = e is ApiFailure ? (e.status == 403 ? 'Нужен сеанс администратора: выйдите из админ-панели и войдите снова.' : e.message) : const ApiFailure('protocol').message);
      if (attempts++ < 5) retry = Timer(const Duration(seconds: 3), () { if (mounted && data == null) load(); });
    }
  }

  Future<void> set(bool on) => widget.mutate(() async {
    await widget.api.request('/api/guest-pairing', body: {'enabled': on});
    if (active) await load();
  });

  @override
  Widget build(BuildContext context) {
    final on = data?['enabled'] == true;
    final mb = ((data?['limits']?['max_file_bytes'] ?? 31457280) as num) ~/ (1 << 20);
    final perDay = data?['limits']?['max_files_per_day'] ?? 20;
    final volume = ((data?['limits']?['max_bytes_per_day'] ?? 314572800) as num) ~/ (1 << 20);
    return PanelStack(
      children: [
        const UiText('Сопряжение без QR', style: TextStyle(fontSize: 22)),
        if (error != null) UiText(error!),
        const UiText(
          'Новые клиенты без QR получают гостевой доступ автоматически (по умолчанию это включено, вы можете выключить). Гость: без групп и без выхода в интернет через узел; личная переписка, голосовые сообщения и аудиозвонки доступны (звонок до 15 минут, до 60 минут в сутки), видеозвонков нет. Свой статус пользователя гость запрашивает у вас, с текстом, в разделе «Жалобы».',
        ),
        PanelCheck(
          value: on,
          label: 'Разрешить сопряжение без QR',
          onChanged: widget.busy || data == null ? null : set,
        ),
        if (data != null) ...[
          UiText('Гостей сейчас: ${data!['guests']} из ${data!['max_guests']}'),
          UiText('Файлы гостя: не больше $mb МБ, не больше $perDay файлов и $volume МБ в сутки; хранение до 24 часов.'),
          const UiText(
            'Адрес гостя узел не записывает и не показывает, но технически видит его при прямом соединении. Гость, молчавший сутки, удаляется.',
          ),
        ],
      ],
    );
  }
}

/// Срок хранения почты между устройствами на узле (шифртекст «от кого и кому»): чем короче, тем меньше следов у того, кто завладел узлом.
class MailPolicyPanel extends StatefulWidget {
  final Api api;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const MailPolicyPanel({super.key, required this.api, required this.busy, required this.isActive, required this.mutate});
  @override
  State<MailPolicyPanel> createState() => _MailPolicyPanelState();
}

class _MailPolicyPanelState extends State<MailPolicyPanel> {
  final days = TextEditingController();
  Map<String, dynamic>? data;
  String? error, saved;
  int attempts = 0;
  Timer? retry;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    retry?.cancel();
    days.dispose();
    super.dispose();
  }

  /// Понятный текст: сеанс администратора мог закончиться (узел перезапущен, прошло 15 минут) — это не «протокол».
  String describe(Object e) {
    if (e is ApiFailure) {
      if (e.status == 403) return 'Нужен сеанс администратора: выйдите из админ-панели и войдите снова.';
      return e.message;
    }
    return const ApiFailure('protocol').message;
  }

  /// Первая загрузка не должна оставлять поле выключенным навсегда: результат принимается, пока панель на экране (а не только пока страница
  /// «активна» в смысле опроса), а при сбое загрузка повторяется несколько раз и есть кнопка «Повторить».
  Future<void> load() async {
    retry?.cancel();
    try {
      final d = await widget.api.request('/api/mail-policy');
      if (!mounted) return;
      setState(() { data = d; days.text = '${d['days']}'; error = null; attempts = 0; });
    } catch (e) {
      if (!mounted) return;
      setState(() => error = describe(e));
      if (attempts++ < 5) retry = Timer(const Duration(seconds: 3), () { if (mounted && data == null) load(); });
    }
  }

  Future<void> save() async {
    final n = int.tryParse(days.text.trim());
    final min = (data?['min'] ?? 1) as int, max = (data?['max'] ?? 30) as int;
    if (n == null || n < min || n > max) {
      setState(() { error = 'Срок хранения почты: от $min до $max суток.'; saved = null; });
      return;
    }
    await widget.mutate(() async {
      try {
        await widget.api.request('/api/mail-policy', body: {'days': n});
      } catch (e) {
        if (mounted) setState(() { error = describe(e); saved = null; });
        return;
      }
      if (mounted) {
        setState(() { error = null; saved = 'Срок хранения почты сохранён.'; });
        await load();
      }
    });
  }

  @override
  Widget build(BuildContext context) => PanelStack(
    children: [
      const UiText('Срок хранения почты', style: TextStyle(fontSize: 22)),
      if (error != null) UiText(error!),
      if (saved != null) UiText(saved!),
      if (data == null) SolidButton('Повторить загрузку', onPressed: () { attempts = 0; load(); }),
      const UiText('Сообщения между устройствами лежат на узле (шифртекстом), пока получатель не заберёт, но не дольше этого срока. Короче срок — меньше следов, если узел попадёт в чужие руки; длиннее — дольше ждёт телефон, который давно не выходил на связь. Гостям срок всегда сутки.'),
      TextField(
        controller: days,
        // поле доступно всегда, пока не идёт сохранение: пределы по умолчанию 1–30 до загрузки, а сбой загрузки виден текстом выше
        enabled: !widget.busy,
        keyboardType: TextInputType.number,
        decoration: InputDecoration(labelText: tr(context, 'Суток (1–30)'), border: const OutlineInputBorder(), contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 14)),
      ),
      SolidButton('Сохранить срок', onPressed: widget.busy ? null : save),
    ],
  );
}

/// Экстренное стирание закрытой среды (история ПК, группы ПК, ключи собеседника, пул прокси): без пароля, но только со словом-подтверждением.
class PanicPanel extends StatefulWidget {
  static const word = 'СТЕРЕТЬ', wordEn = 'DELETE';
  /// Принимаются оба слова в любом регистре: на английском интерфейсе показывается DELETE (на клавиатуре может не быть кириллицы).
  static bool confirmed(String typed) {
    final t = typed.trim().toUpperCase();
    return t == word || t == wordEn;
  }

  final Api api;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const PanicPanel({super.key, required this.api, required this.busy, required this.isActive, required this.mutate});
  @override
  State<PanicPanel> createState() => _PanicPanelState();
}

class _PanicPanelState extends State<PanicPanel> {
  String? done;
  bool get active => mounted && widget.isActive();

  Future<void> erase() async {
    final typed = await showDialog<String>(context: context, builder: (_) => const _PanicWordDialog());
    if (typed == null || !active) return;
    if (!PanicPanel.confirmed(typed)) {
      setState(() => done = 'Слово не совпало: ничего не стёрто.');
      return;
    }
    await widget.mutate(() async {
      await widget.api.request('/api/vault/panic_erase', body: {'confirm': typed.trim()});
      if (active) setState(() => done = 'Закрытая среда стёрта.');
    });
  }

  @override
  Widget build(BuildContext context) => PanelStack(
    children: [
      const UiText('Экстренное стирание', style: TextStyle(fontSize: 22)),
      const UiText('Быстро и без пароля стирает закрытую среду этого компьютера. Нужно только подтвердить словом, чтобы не нажать случайно.'),
      if (done != null) UiText(done!),
      SolidButton('Стереть закрытую среду', tone: SolidTone.danger, onPressed: widget.busy ? null : erase),
    ],
  );
}

/// Окно подтверждения экстренного стирания: возвращает введённое слово (проверяет вызывающий) или null при отмене.
class _PanicWordDialog extends StatefulWidget {
  const _PanicWordDialog();
  @override
  State<_PanicWordDialog> createState() => _PanicWordDialogState();
}

class _PanicWordDialogState extends State<_PanicWordDialog> {
  final word = TextEditingController();
  @override
  void dispose() {
    word.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const UiText('Стереть закрытую среду?'),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const UiText('Будут стёрты ключи и зашифрованные данные закрытой среды на этом компьютере: история, группы, ключи собеседника, пул прокси. Вернуть их нельзя ни по паролю, ни по сид-фразе. Личность узла и файл резервной копии останутся.'),
        const SizedBox(height: 12),
        TextField(
          controller: word,
          autofocus: true,
          decoration: InputDecoration(labelText: tr(context, 'Введите слово СТЕРЕТЬ'), border: const OutlineInputBorder()),
          onSubmitted: (_) => Navigator.pop(context, word.text),
        ),
      ],
    ),
    actions: [
      TextButton(onPressed: () => Navigator.pop(context), child: const UiText('Отмена')),
      SolidButton('Стереть', tone: SolidTone.danger, onPressed: () => Navigator.pop(context, word.text)),
    ],
  );
}

class AboutPanel extends StatelessWidget {
  const AboutPanel({super.key});
  @override
  Widget build(BuildContext context) => const PanelStack(
    children: [
      UiText('О приложении', style: TextStyle(fontSize: 24)),
      UiText('YANDI Desktop · 0.1.0'),
      UiText(
        'Локальная оболочка узла YANDI. Ключи закрытой среды и криптография находятся у демона. Связь через Unix-сокет.',
      ),
      UiText(
        'Версия демона не предоставляется API. Веб-интерфейс пока сохранён.',
      ),
    ],
  );
}
