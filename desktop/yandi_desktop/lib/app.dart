import 'dart:async';
import 'package:flutter/material.dart';
import 'domain/states.dart';
import 'transport/api.dart';

class DesktopApp extends StatefulWidget {
  final Api api;
  const DesktopApp({super.key, required this.api});
  @override
  State<DesktopApp> createState() => _DesktopAppState();
}

class _DesktopAppState extends State<DesktopApp> with WidgetsBindingObserver {
  final navigator = GlobalKey<NavigatorState>();
  AuthStage? auth;
  VaultState? vault;
  bool connected = false, entered = false, busy = false, visible = true;
  bool polling = false, lockUnconfirmed = false;
  int epoch = 0, tick = 0, formVersion = 0, operation = 0;
  String? error, notice;
  String page = 'Панель';
  Map<String, dynamic>? status, speed;
  List<dynamic> nodes = [];
  Timer? timer;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    checkAuth();
    startTimer();
  }

  void startTimer() {
    timer?.cancel();
    if (visible)
      timer = Timer.periodic(const Duration(seconds: 1), (_) => poll());
  }

  void wipe() {
    epoch++;
    formVersion++;
    widget.api.cancel();
    status = speed = null;
    nodes = [];
    vault = null;
  }

  void disconnect(ApiFailure failure) {
    wipe();
    connected = entered = false;
    error = failure.message;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    visible = state == AppLifecycleState.resumed;
    timer?.cancel();
    wipe();
    if (mounted) setState(() {});
    if (visible) {
      checkAuth(preserveEntry: entered);
      startTimer();
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    widget.api.cancel();
    super.dispose();
  }

  Future<void> checkAuth({bool preserveEntry = false}) async {
    final task = ++operation;
    final own = ++epoch;
    if (mounted)
      setState(() {
        busy = true;
        error = null;
      });
    try {
      final next = parseAuth(await widget.api.request('/api/auth/state'));
      if (!mounted || own != epoch) return;
      setState(() {
        auth = next;
        connected = true;
        entered = preserveEntry && next == AuthStage.login;
      });
      if (entered && !lockUnconfirmed) await refreshVault(own);
    } catch (e) {
      if (mounted && own == epoch)
        setState(
          () => disconnect(e is ApiFailure ? e : const ApiFailure('protocol')),
        );
    } finally {
      if (mounted && task == operation)
        setState(() {
          busy = false;
        });
    }
  }

  Future<void> refreshVault(int own) async {
    final next = VaultState.fromJson(
      await widget.api.request('/api/vault/status'),
    );
    if (!mounted || own != epoch) return;
    if (vault?.unlocked == true && !next.unlocked) {
      formVersion++;
      status = speed = null;
      nodes = [];
    }
    setState(() {
      vault = next;
      lockUnconfirmed = false;
    });
  }

  Future<void> enter() async {
    if (busy) return;
    setState(() {
      entered = true;
    });
    await poll(force: true);
  }

  Future<void> poll({bool force = false}) async {
    if (!visible ||
        !connected ||
        !entered ||
        busy ||
        polling ||
        lockUnconfirmed)
      return;
    polling = true;
    final own = epoch;
    final current = tick++;
    try {
      // Re-read startup flags, including after a daemon restart.
      if (force || current % 5 == 0) {
        final next = parseAuth(await widget.api.request('/api/auth/state'));
        if (!mounted || own != epoch) return;
        if (next != AuthStage.login) {
          setState(() {
            wipe();
            auth = next;
            entered = false;
          });
          return;
        }
      }
      await refreshVault(own);
      if (!mounted || own != epoch || page != 'Панель') return;
      if (force || current % 5 == 0) {
        final result = await widget.api.request('/api/status');
        if (!mounted || own != epoch) return;
        setState(() {
          status = result;
        });
      }
      if (force || current % 2 == 0) {
        final result = await widget.api.request('/api/speed');
        if (!mounted || own != epoch) return;
        setState(() {
          speed = result;
        });
      }
      if (force || current % 10 == 0) {
        final result = await widget.api.request('/api/nodes');
        if (result['nodes'] is! List) throw const ApiFailure('protocol');
        if (!mounted || own != epoch) return;
        setState(() {
          nodes = result['nodes'] as List;
        });
      }
    } catch (e) {
      if (mounted && own == epoch)
        setState(
          () => disconnect(e is ApiFailure ? e : const ApiFailure('protocol')),
        );
    } finally {
      polling = false;
    }
  }

  Future<void> act(String path, Map<String, dynamic> body) async {
    if (busy || !visible) return;
    final task = ++operation;
    final isLock = path == '/api/vault/lock';
    final own = ++epoch;
    widget.api.cancel();
    setState(() {
      busy = true;
      error = notice = null;
      formVersion++;
      if (isLock) {
        status = speed = null;
        nodes = [];
        vault = null;
        lockUnconfirmed = true;
      }
    });
    try {
      final pending = widget.api.request(path, body: body);
      body.clear();
      await pending;
      if (!mounted || own != epoch) return;
      if (path == '/api/auth/setup') {
        setState(() {
          notice = 'Ключи созданы. Нода запускается…';
        });
        final until = DateTime.now().add(const Duration(seconds: 120));
        var ready = false;
        while (DateTime.now().isBefore(until) && mounted && own == epoch) {
          await Future<void>.delayed(const Duration(seconds: 1));
          try {
            final next = parseAuth(await widget.api.request('/api/auth/state'));
            if (!mounted || own != epoch) return;
            if (next != AuthStage.setup) {
              setState(() {
                auth = next;
                entered = false;
                notice = null;
              });
              ready = true;
              break;
            }
          } on ApiFailure {
            /* Only GET is retried during daemon handover. */
          }
        }
        if (!ready) throw const ApiFailure('timeout');
      } else if (path.startsWith('/api/auth/')) {
        final next = parseAuth(await widget.api.request('/api/auth/state'));
        if (!mounted || own != epoch) return;
        setState(() {
          auth = next;
          entered = false;
          notice = 'Доступ восстановлен.';
        });
      } else {
        await refreshVault(own);
        if (mounted && own == epoch)
          setState(() {
            notice = 'Действие выполнено.';
          });
      }
    } catch (e) {
      if (!mounted || own != epoch) return;
      final failure = e is ApiFailure ? e : const ApiFailure('protocol');
      setState(() {
        if (failure.kind == 'disconnected' || failure.kind == 'timeout') {
          disconnect(failure);
        } else {
          error = failure.message;
        }
        if (isLock) lockUnconfirmed = true;
      });
    } finally {
      body.clear();
      if (mounted && task == operation)
        setState(() {
          busy = false;
        });
    }
  }

  Widget button(String label, VoidCallback action) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 6),
    child: FilledButton(onPressed: busy ? null : action, child: Text(label)),
  );
  Widget dashboard() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const Text('Статус узла', style: TextStyle(fontSize: 22)),
      if (status == null)
        const Text('Загрузка статуса…')
      else ...[
        for (final item in const {
          'is_local': 'Локальный узел',
          'status': 'Статус',
          'short_id': 'Short ID',
          'cid': 'CID',
          'node_id': 'ID узла',
          'role': 'Роль',
          'mode': 'Режим',
          'nat_status': 'NAT',
          'external_ip': 'Внешний IP',
          'virtual_ipv6': 'Виртуальная IPv6',
          'ipv6_short': 'Короткая IPv6',
          'discovery_port': 'Порт обнаружения',
          'data_port': 'Порт данных',
          'web_port': 'Веб-порт',
          'active_proxies': 'Активные прокси',
        }.entries)
          SelectableText('${item.value}: ${status![item.key] ?? '—'}'),
      ],
      Wrap(
        spacing: 12,
        children: [
          button('Запустить узел', () => act('/api/node/start', {})),
          button('Остановить узел', () => act('/api/node/stop', {})),
        ],
      ),
      const Divider(),
      const Text('Скорость', style: TextStyle(fontSize: 22)),
      if (speed == null)
        const Text('Загрузка метрик…')
      else
        for (final item in const {
          'rx_speed': 'Приём, Б/с',
          'tx_speed': 'Отправка, Б/с',
          'peer_rx_estimate': 'Оценка приёма соседа, Б/с',
          'avg_rtt_ms': 'RTT, мс',
          'path0_loss_incoming': 'Path0: входящие потери (диагностика)',
          'peer_path0_loss': 'Path0 соседа (диагностика)',
          'clone_hit_pct': 'Попадания клонов, %',
          'wagon_drop_crc_pct': 'Отброшено + CRC, %',
          'depot_bytes': 'Депо, байт',
          'active_trains': 'Активные поезда',
          'evictions_delta': 'Вытеснения за интервал',
          'evictions_total': 'Вытеснения всего',
        }.entries)
          Text('${item.value}: ${speed![item.key] ?? '—'}'),
      const Divider(),
      const Text('Узлы', style: TextStyle(fontSize: 22)),
      if (nodes.isEmpty) const Text('Обнаруженных узлов нет.'),
      for (final node in nodes.whereType<Map>())
        Card(
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final entry in const {
                  'short_id': 'Short ID',
                  'hostname': 'Имя узла',
                  'source': 'Источник обнаружения',
                  'role': 'Роль',
                  'nat_status': 'NAT',
                  'connected': 'Соединение (по данным демона)',
                  'addresses': 'Адреса',
                  'admin_port': 'Порт управления',
                  'discovery_port': 'Порт обнаружения',
                  'data_port': 'Порт данных',
                }.entries)
                  if (node.containsKey(entry.key))
                    SelectableText('${entry.value}: ${node[entry.key]}'),
                const Text('Подключение узла в текущем API — заглушка.'),
              ],
            ),
          ),
        ),
      const Divider(),
      const Text('Шлюз', style: TextStyle(fontSize: 22)),
      const Text('Запуск и остановка через API пока не реализованы демоном.'),
      Wrap(
        spacing: 12,
        children: [
          button('Запустить шлюз', () => act('/api/gateway/start', {})),
          button('Остановить шлюз', () => act('/api/gateway/stop', {})),
        ],
      ),
    ],
  );

  Widget vaultPanel() {
    final v = vault;
    if (v == null)
      return Text(
        lockUnconfirmed
            ? 'Состояние блокировки не подтверждено'
            : 'Проверка закрытой среды…',
      );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('Закрытая среда', style: TextStyle(fontSize: 22)),
        Text(v.label),
        if (v.migrationPending)
          const Text(
            'Перенос части старых данных не завершён и повторится при входе.',
          ),
        const Text(
          'Пароль владельца закрытой среды отличается от пароля входа и мастер-пароля.',
        ),
        if (!v.configured)
          SecretForm(
            key: ValueKey('vault-setup-$formVersion'),
            title: 'Создать закрытую среду',
            labels: const [
              'Пароль владельца',
              'Повтор пароля',
              'Четыре секретных слова (необязательно)',
            ],
            busy: busy,
            submit: (values) async {
              if (values[0] != values[1])
                throw const FormatException('Пароли не совпадают');
              if (values[0].runes.length < 6)
                throw const FormatException('Пароль — не короче 6 символов');
              final phrase = values[2].trim();
              if (phrase.isNotEmpty && phrase.split(RegExp(r'\s+')).length != 4)
                throw const FormatException('Введите четыре секретных слова');
              final operationEpoch = epoch;
              final payload = <String, dynamic>{
                'password': values[0],
                'phrase': phrase.isEmpty ? null : phrase,
              };
              if (phrase.isEmpty) {
                final answer = await showDialog<bool>(
                  context: navigator.currentContext!,
                  builder: (context) => AlertDialog(
                    title: const Text('Без секретных слов'),
                    content: const Text(
                      'Без секретных слов забытый пароль не восстановить. Продолжить без слов?',
                    ),
                    actions: [
                      TextButton(
                        onPressed: () => Navigator.pop(context, false),
                        child: const Text('Отмена'),
                      ),
                      TextButton(
                        onPressed: () => Navigator.pop(context, true),
                        child: const Text('Продолжить'),
                      ),
                    ],
                  ),
                );
                if (answer != true || !visible || operationEpoch != epoch) {
                  payload.clear();
                  return;
                }
              }
              await act('/api/vault/setup', payload);
            },
          ),
        if (v.configured && !v.unlocked) ...[
          SecretForm(
            key: ValueKey('unlock-$formVersion'),
            title: 'Разблокировать',
            labels: const ['Пароль владельца'],
            busy: busy,
            submit: (values) =>
                act('/api/vault/unlock', {'password': values[0]}),
          ),
          SecretForm(
            key: ValueKey('recover-$formVersion'),
            title: 'Восстановить словами',
            labels: const [
              'Четыре секретных слова',
              'Новый пароль',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (values) {
              validateNew(values[1], values[2], minimum: 6);
              return act('/api/vault/recover', {
                'phrase': values[0],
                'new_password': values[1],
              });
            },
          ),
        ],
        if (v.unlocked) ...[
          button('Запереть', () => act('/api/vault/lock', {})),
          SecretForm(
            key: ValueKey('change-$formVersion'),
            title: 'Сменить пароль',
            labels: const [
              'Старый пароль',
              'Новый пароль',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (values) {
              validateNew(values[1], values[2], minimum: 6);
              return act('/api/vault/change_password', {
                'old_password': values[0],
                'new_password': values[1],
              });
            },
          ),
        ],
        Text(
          'Автоблокировка: ${v.autoLockSecs} с. Таймером управляет демон; опрос не считается активностью.',
        ),
        TimeoutForm(
          key: ValueKey('timeout-$formVersion'),
          busy: busy,
          initial: v.autoLockSecs,
          submit: (secs) => act('/api/vault/auto_lock', {'secs': secs}),
        ),
      ],
    );
  }

  static void validateNew(String a, String b, {int minimum = 8}) {
    if (a.runes.length < minimum)
      throw FormatException('Новый пароль — не короче $minimum символов');
    if (a != b) throw const FormatException('Пароли не совпадают');
  }

  Widget content() {
    if (!connected)
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('Нет связи с демоном', style: TextStyle(fontSize: 24)),
          const Text(
            'Запустите YANDI под вашим пользователем. Оболочка подключается только к локальному Unix-сокету; интернет для этого не нужен.',
          ),
          if (lockUnconfirmed)
            const Text('Состояние блокировки не подтверждено'),
          button('Повторить подключение', () => checkAuth()),
        ],
      );
    if (auth == AuthStage.setup)
      return SecretForm(
        key: ValueKey('setup-$formVersion'),
        title: 'Создать и запустить ноду',
        description:
            'Первичная настройка. Запишите мастер-пароль на бумаге: он нужен для восстановления.',
        labels: const [
          'Пароль входа',
          'Повтор пароля входа',
          'Мастер-пароль',
          'Повтор мастер-пароля',
        ],
        busy: busy,
        submit: (v) {
          validateNew(v[0], v[1]);
          if (v[2].runes.length < 12)
            throw const FormatException(
              'Мастер-пароль — не короче 12 символов',
            );
          if (v[2] != v[3])
            throw const FormatException('Мастер-пароли не совпадают');
          if (v[0] == v[2])
            throw const FormatException(
              'Пароль входа и мастер-пароль должны отличаться',
            );
          return act('/api/auth/setup', {
            'login_password': v[0],
            'login_password_repeat': v[1],
            'master_password': v[2],
            'master_password_repeat': v[3],
          });
        },
      );
    if (!entered || auth == AuthStage.recovery)
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            auth == AuthStage.recovery
                ? 'Требуется восстановление'
                : 'Доступ к ноде',
            style: const TextStyle(fontSize: 24),
          ),
          if (auth == AuthStage.recovery)
            const Text(
              'Проблема привязки ключей к устройству. Для перепривязки используйте yandi-keys recover в терминале; оболочка не выполняет перепривязку.',
            )
          else ...[
            const Text(
              'Локальный вход разрешён демоном по uid владельца Unix-сокета. Пароль браузерного входа здесь не требуется. Закрытая среда открывается отдельным паролем.',
            ),
            button('Войти', enter),
          ],
          SecretForm(
            key: ValueKey('auth-recover-$formVersion'),
            title: 'Восстановить доступ',
            labels: const [
              'Мастер-пароль или код восстановления',
              'Новый пароль входа',
              'Повтор нового пароля',
            ],
            busy: busy,
            submit: (v) {
              validateNew(v[1], v[2]);
              return act('/api/auth/recover', {
                'recovery_code': v[0],
                'new_login_password': v[1],
                'remember_me': false,
              });
            },
          ),
        ],
      );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          spacing: 12,
          children: [
            for (final label in const ['Панель', 'Закрытая среда'])
              OutlinedButton(
                onPressed: busy
                    ? null
                    : () {
                        setState(() {
                          page = label;
                          formVersion++;
                        });
                        poll(force: true);
                      },
                child: Text(label),
              ),
            button('Выйти из оболочки', () {
              setState(() {
                wipe();
                entered = false;
                notice = error = null;
              });
            }),
          ],
        ),
        const Text(
          'Выход из оболочки не запирает закрытую среду. Используйте «Запереть».',
        ),
        if (lockUnconfirmed) ...[
          const Text('Состояние блокировки не подтверждено'),
          button('Повторить блокировку', () => act('/api/vault/lock', {})),
        ] else if (page == 'Панель')
          dashboard()
        else
          vaultPanel(),
      ],
    );
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    navigatorKey: navigator,
    title: 'YANDI',
    debugShowCheckedModeBanner: false,
    theme: ThemeData(
      colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xff3154a0)),
      useMaterial3: true,
    ),
    home: Scaffold(
      appBar: AppBar(title: const Text('YANDI · локальная оболочка')),
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Align(
            alignment: Alignment.topCenter,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 960),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (busy) const LinearProgressIndicator(),
                  if (error != null)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 16),
                      child: Text(
                        error!,
                        style: const TextStyle(color: Colors.red),
                      ),
                    ),
                  if (notice != null)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 16),
                      child: Text(notice!),
                    ),
                  content(),
                ],
              ),
            ),
          ),
        ),
      ),
    ),
  );
}

class SecretForm extends StatefulWidget {
  final String title;
  final String? description;
  final List<String> labels;
  final bool busy;
  final Future<void> Function(List<String>) submit;
  const SecretForm({
    super.key,
    required this.title,
    this.description,
    required this.labels,
    required this.busy,
    required this.submit,
  });
  @override
  State<SecretForm> createState() => _SecretFormState();
}

class _SecretFormState extends State<SecretForm> {
  late final fields = List.generate(
    widget.labels.length,
    (_) => TextEditingController(),
  );
  bool sending = false;
  String? error;
  Future<void> send() async {
    if (sending || widget.busy) return;
    setState(() {
      sending = true;
      error = null;
    });
    final bytes = fields.map((f) => SecretBytes(f.text)).toList();
    for (final field in fields) {
      field.clear();
    }
    final values = bytes.map((b) => b.decode()).toList();
    try {
      final pending = widget.submit(values);
      values.fillRange(0, values.length, '');
      for (final secret in bytes) {
        secret.clear();
      }
      await pending;
    } on FormatException catch (e) {
      if (mounted)
        setState(() {
          error = e.message;
        });
    } finally {
      values.fillRange(0, values.length, '');
      for (final secret in bytes) {
        secret.clear();
      }
      if (mounted)
        setState(() {
          sending = false;
        });
    }
  }

  @override
  void dispose() {
    for (final field in fields) {
      field.clear();
      field.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 16),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(widget.title, style: const TextStyle(fontSize: 20)),
        if (widget.description != null) Text(widget.description!),
        for (var i = 0; i < fields.length; i++)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: TextField(
              controller: fields[i],
              obscureText: true,
              enableSuggestions: false,
              autocorrect: false,
              autofillHints: const [],
              enableIMEPersonalizedLearning: false,
              enabled: !sending && !widget.busy,
              decoration: InputDecoration(
                labelText: widget.labels[i],
                border: const OutlineInputBorder(),
              ),
              onSubmitted: (_) => send(),
            ),
          ),
        if (error != null)
          Text(error!, style: const TextStyle(color: Colors.red)),
        FilledButton(
          onPressed: sending || widget.busy ? null : send,
          child: Text(sending ? 'Подождите…' : widget.title),
        ),
      ],
    ),
  );
}

class TimeoutForm extends StatefulWidget {
  final bool busy;
  final int initial;
  final Future<void> Function(int) submit;
  const TimeoutForm({
    super.key,
    required this.busy,
    required this.initial,
    required this.submit,
  });
  @override
  State<TimeoutForm> createState() => _TimeoutFormState();
}

class _TimeoutFormState extends State<TimeoutForm> {
  late final field = TextEditingController(text: widget.initial.toString());
  String? error;
  @override
  void dispose() {
    field.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      TextField(
        controller: field,
        keyboardType: TextInputType.number,
        decoration: const InputDecoration(
          labelText:
              'Автоблокировка, секунды (0 — отключена, максимум 2592000)',
        ),
      ),
      if (error != null) Text(error!),
      FilledButton(
        onPressed: widget.busy
            ? null
            : () {
                final secs = int.tryParse(field.text);
                if (secs == null || secs < 0 || secs > 2592000) {
                  setState(() {
                    error = 'Введите число от 0 до 2592000';
                  });
                  return;
                }
                setState(() {
                  error = null;
                });
                widget.submit(secs);
              },
        child: const Text('Сохранить автоблокировку'),
      ),
    ],
  );
}
