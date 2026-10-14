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
  final cap = TextEditingController();
  String exit = 'trusted';
  Map<String, dynamic>? pool;
  String? error;
  bool loading = false;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    refresh();
  }

  @override
  void dispose() {
    pool = null;
    cap.clear();
    cap.dispose();
    super.dispose();
  }

  Future<void> refresh() async {
    if (!active || loading) return;
    loading = true;
    try {
      final policy = await widget.api.request('/api/exit/settings');
      final proxies = await widget.api.request('/api/upstream-proxies');
      if (!active) return;
      ShellRepository.list(proxies, 'proxies');
      if (!['off', 'trusted', 'all'].contains(policy['mode']))
        throw const ApiFailure('protocol');
      setState(() {
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
      if (loading) const LinearProgressIndicator(),
      if (error != null) UiText(error!),
      button('Обновить настройки', refresh),
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
