import 'theme.dart';
import 'package:flutter/material.dart';
import 'transport/api.dart';
import 'domain/states.dart';
import 'domain/shell_repository.dart';
import 'preferences.dart';
import 'groups.dart';

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
  static const portLabels = [
    'Discovery порт',
    'Data порт',
    'Mobile Gateway порт',
    'Mobile P2P порт',
    'HTTP Proxy порт',
    'Web UI порт',
  ];
  static const portHelp = [
    'Порт для обнаружения пиров (Hello packets)',
    'Порт для передачи данных между нодами (зашифрованные wagons)',
    'Порт шлюза для телефонов',
    'Порт P2P для телефонов',
    'Порт локального HTTP-прокси',
    'Порт веб-интерфейса узла',
  ];
  static const ports = [
    'discovery_port',
    'data_port',
    'mobile_gateway',
    'mobile_p2p',
    'http_proxy',
    'web_ui',
  ];
  final fields = List.generate(7, (_) => TextEditingController());
  final cap = TextEditingController();
  String level = 'info', exit = 'trusted';
  Map<String, dynamic>? pool;
  String? error;
  bool restart = false, loading = false;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    refresh();
  }

  @override
  void dispose() {
    pool = null;
    for (final f in [...fields, cap]) {
      f.clear();
      f.dispose();
    }
    super.dispose();
  }

  Future<void> refresh() async {
    if (!active || loading) return;
    loading = true;
    try {
      final settings = await widget.api.request('/api/settings');
      final policy = await widget.api.request('/api/exit/settings');
      final proxies = await widget.api.request('/api/upstream-proxies');
      if (!active) return;
      ShellRepository.list(proxies, 'proxies');
      if (settings['network'] is! Map ||
          settings['server'] is! Map ||
          !['off', 'trusted', 'all'].contains(policy['mode']))
        throw const ApiFailure('protocol');
      setState(() {
        for (var i = 0; i < ports.length; i++) {
          fields[i].text = '${settings['network'][ports[i]]}';
        }
        fields.last.text = '${settings['server']['bind_address']}';
        level = '${settings['server']['log_level']}';
        exit = policy['mode'];
        pool = proxies;
        cap.text = '${proxies['max_clients']}';
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
  Future<void> saveNetwork() async {
    final values = fields.take(6).map((f) => int.tryParse(f.text)).toList();
    if (values.any((p) => p == null || p < 1 || p > 65535) ||
        fields.last.text.trim().isEmpty) {
      setState(() => error = 'Порты: 1–65535. Укажите IP bind.');
      return;
    }
    await widget.mutate(() async {
      final result = await widget.api.request(
        '/api/settings',
        method: 'PUT',
        body: {
          'network': {for (var i = 0; i < 6; i++) ports[i]: values[i]},
          'server': {
            'bind_address': fields.last.text.trim(),
            'log_level': level,
          },
        },
      );
      if (active) setState(() => restart = result['requires_restart'] == true);
    });
  }

  Future<void> createProxy() async {
    final body = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => const ProxyForm(),
    );
    if (!active || body == null) return;
    try {
      await sealedAction(() async {
        await widget.api.request('/api/upstream-proxies', body: body);
      });
    } finally {
      body.clear();
    }
  }

  Future<void> editProxy(Map<String, dynamic> proxy) async {
    final values = await showDialog<List<String>>(
      context: context,
      builder: (_) => GroupForm(
        title: 'Лимит прокси',
        labels: const ['Клиентов (0 — общий предел)'],
        initial: ['${proxy['max_clients'] ?? 0}'],
      ),
    );
    if (!active || values == null) return;
    final limit = int.tryParse(values.single);
    if (limit == null || limit < 0 || limit > 1000) {
      setState(() => error = 'Лимит: 0–1000.');
      return;
    }
    await sealedAction(() async {
      await widget.api.request(
        '/api/upstream-proxies/${ShellRepository(widget.api).segment('${proxy['id']}')}',
        body: {'max_clients': limit},
      );
    });
  }

  Widget button(String text, Future<void> Function() action) => TextButton(
    onPressed: widget.busy || loading ? null : action,
    child: UiText(text),
  );
  @override
  Widget build(BuildContext context) => PanelStack(
    children: [
      const UiText('Сеть', style: TextStyle(fontSize: 22)),
      if (loading) const LinearProgressIndicator(),
      if (error != null) UiText(error!),
      if (restart)
        const UiText('Нужен перезапуск узла для применения настроек.'),
      button('Обновить настройки', refresh),
      for (var i = 0; i < fields.length; i++)
        Align(
          alignment: Alignment.centerLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: TextField(
              controller: fields[i],
              enabled: !widget.busy && !loading,
              decoration: InputDecoration(
                labelText: i < 6
                    ? tr(context, portLabels[i])
                    : tr(context, 'Адрес bind'),
                helperText: i < 6
                    ? '${tr(context, portHelp[i])} (${ports[i]})'
                    : null,
              ),
            ),
          ),
        ),
      DropdownButton<String>(
        value: level,
        items: [
          for (final v in ['error', 'warn', 'info', 'debug', 'trace'])
            DropdownMenuItem(value: v, child: UiText(v)),
        ],
        onChanged: widget.busy ? null : (v) => setState(() => level = v!),
      ),
      button('Сохранить порты и журнал', saveNetwork),
      const UiText(
        'Выход в интернет через ваш узел',
        style: TextStyle(fontSize: 22),
      ),
      const UiText('Кто может выходить в интернет через этот узел'),
      DropdownButton<String>(
        value: exit,
        items: [
          for (final e in {
            'off': 'Никто',
            'trusted': 'Доверенные узлы',
            'all': 'Все узлы',
          }.entries)
            DropdownMenuItem(value: e.key, child: UiText(e.value)),
        ],
        onChanged: widget.busy ? null : (v) => setState(() => exit = v!),
      ),
      button(
        'Сохранить политику выхода',
        () => widget.mutate(() async {
          await widget.api.request('/api/exit/settings', body: {'mode': exit});
        }),
      ),
      const UiText(
        'Внешние прокси для выхода в интернет',
        style: TextStyle(fontSize: 22),
      ),
      const UiText(
        'Изменения пула требуют открытой настроенной закрытой среды. Пароли хранит только демон в запечатанном файле.',
      ),
      const UiText(
        'API правки адреса и пароля существующего прокси отсутствует. Доступны включение и лимит.',
      ),
      button('Добавить внешний прокси', createProxy),
      if (pool != null) ...[
        CheckboxListTile(
          controlAffinity: ListTileControlAffinity.leading,
          title: const UiText('Включить пул'),
          value: pool!['enabled'] == true,
          onChanged: widget.busy
              ? null
              : (v) => sealedAction(() async {
                  await widget.api.request(
                    '/api/upstream-proxies/settings',
                    body: {'enabled': v!},
                  );
                }),
        ),
        CheckboxListTile(
          controlAffinity: ListTileControlAffinity.leading,
          title: const UiText('Строго соблюдать лимит клиентов'),
          value: pool!['strict'] == true,
          onChanged: widget.busy
              ? null
              : (v) => sealedAction(() async {
                  await widget.api.request(
                    '/api/upstream-proxies/settings',
                    body: {'strict': v!},
                  );
                }),
        ),
        TextField(
          controller: cap,
          enabled: !widget.busy,
          decoration: InputDecoration(
            labelText: tr(context, 'Общий лимит клиентов'),
          ),
        ),
        button('Сохранить лимит пула', () async {
          final n = int.tryParse(cap.text);
          if (n == null || n < 1 || n > 1000) {
            setState(() => error = 'Лимит: 1–1000.');
            return;
          }
          await sealedAction(() async {
            await widget.api.request(
              '/api/upstream-proxies/settings',
              body: {'max_clients': n},
            );
          });
        }),
        for (final p in ShellRepository.list(pool!, 'proxies'))
          Card(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                UiText('${p['kind']} · ${p['host']}:${p['port']}'),
                UiText(
                  '${stateLabel(context, p['state'])} · Клиентов: ${p['clients']} / ${p['cap']}',
                ),
                UiText(
                  p['has_password'] == true
                      ? 'Пароль сохранён демоном'
                      : 'Без пароля',
                ),
                Wrap(
                  children: [
                    button(
                      p['enabled'] == true
                          ? 'Выключить прокси'
                          : 'Включить прокси',
                      () => sealedAction(() async {
                        await widget.api.request(
                          '/api/upstream-proxies/${ShellRepository(widget.api).segment('${p['id']}')}',
                          body: {'enabled': p['enabled'] != true},
                        );
                      }),
                    ),
                    button('Изменить лимит прокси', () => editProxy(p)),
                    button('Удалить внешний прокси', () async {
                      final ok = await showDialog<bool>(
                        context: context,
                        builder: (_) => const GroupConfirm(
                          title: 'Удалить внешний прокси?',
                        ),
                      );
                      if (!active || ok != true) return;
                      await sealedAction(() async {
                        await widget.api.request(
                          '/api/upstream-proxies/${ShellRepository(widget.api).segment('${p['id']}')}',
                          method: 'DELETE',
                        );
                      });
                    }),
                  ],
                ),
              ],
            ),
          ),
      ],
    ],
  );
}

class ProxyForm extends StatefulWidget {
  const ProxyForm({super.key});
  @override
  State<ProxyForm> createState() => _ProxyFormState();
}

class _ProxyFormState extends State<ProxyForm> {
  final fields = List.generate(4, (_) => TextEditingController());
  String kind = 'socks5';
  String? error;
  @override
  void dispose() {
    for (final f in fields) {
      f.clear();
      f.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const UiText('Добавить внешний прокси'),
    content: SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DropdownButton<String>(
            value: kind,
            items: [
              for (final k in ['socks5', 'http_connect'])
                DropdownMenuItem(value: k, child: UiText(k)),
            ],
            onChanged: (v) => setState(() => kind = v!),
          ),
          for (var i = 0; i < fields.length; i++)
            TextField(
              controller: fields[i],
              obscureText: i == 3,
              autocorrect: false,
              enableSuggestions: false,
              decoration: InputDecoration(
                labelText: tr(
                  context,
                  [
                    'Хост прокси',
                    'Порт прокси',
                    'Логин прокси (необязательно)',
                    'Пароль прокси',
                  ][i],
                ),
              ),
            ),
          if (error != null) UiText(error!),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const UiText('Отмена'),
      ),
      FilledButton(
        onPressed: () {
          final port = int.tryParse(fields[1].text);
          if (port == null ||
              port < 1 ||
              port > 65535 ||
              fields[0].text.trim().isEmpty ||
              fields[2].text.isEmpty != fields[3].text.isEmpty) {
            setState(
              () => error =
                  'Укажите хост, порт 1–65535 и оба поля авторизации либо оставьте их пустыми.',
            );
            return;
          }
          final result = <String, dynamic>{
            'kind': kind,
            'host': fields[0].text.trim(),
            'port': port,
            'auth': fields[2].text.isEmpty ? 'none' : 'password',
            'user': fields[2].text,
            'password': fields[3].text,
          };
          for (final f in fields) {
            f.clear();
          }
          Navigator.pop(context, result);
        },
        child: const UiText('Сохранить'),
      ),
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
