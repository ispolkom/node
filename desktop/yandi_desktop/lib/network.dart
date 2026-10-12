import 'dart:async';
import 'package:flutter/material.dart';
import 'domain/network_repository.dart';
import 'domain/states.dart';
import 'transport/api.dart';

class NetworkFeature extends StatefulWidget {
  final Api api;
  final String page;
  final bool Function() isActive;
  final VoidCallback onLocked;
  final void Function(ApiFailure) onFailure;
  const NetworkFeature({
    super.key,
    required this.api,
    required this.page,
    required this.isActive,
    required this.onLocked,
    required this.onFailure,
  });
  @override
  State<NetworkFeature> createState() => _NetworkFeatureState();
}

class _NetworkFeatureState extends State<NetworkFeature> {
  late final repo = NetworkRepository(widget.api);
  final name = TextEditingController(), id = TextEditingController();
  String country = 'RU';
  String? editing, error, notice;
  List<Map<String, dynamic>> rows = [], sessions = [], peers = [];
  Map<String, dynamic>? http, socks, relay;
  bool busy = false, refreshing = false;
  Timer? timer;
  bool get active => mounted && widget.isActive();
  bool get gateways => widget.page == 'Шлюзы';
  @override
  void initState() {
    super.initState();
    refresh();
    timer = Timer.periodic(const Duration(seconds: 5), (_) => refresh());
  }

  @override
  void dispose() {
    timer?.cancel();
    name.clear();
    id.clear();
    name.dispose();
    id.dispose();
    rows = [];
    sessions = [];
    peers = [];
    http = socks = relay = null;
    super.dispose();
  }

  void fail(Object e) {
    if (!active) return;
    final f = e is ApiFailure ? e : const ApiFailure('protocol');
    if (e is FormatException) {
      setState(() => error = e.message);
      return;
    }
    if (['disconnected', 'timeout', 'protocol'].contains(f.kind)) {
      widget.onFailure(f);
      return;
    }
    setState(() => error = f.message);
  }

  Future<void> refresh({bool nodes = false}) async {
    if (!active || refreshing || busy) return;
    refreshing = true;
    try {
      if (gateways) {
        final r = await repo.gateways();
        if (!active) return;
        final h = await repo.proxy('proxy');
        if (!active) return;
        final s = await repo.proxy('socks5');
        if (!active) return;
        setState(() {
          rows = r;
          http = h;
          socks = s;
        });
      } else {
        final r = await repo.relay();
        if (!active) return;
        final s = await repo.sessions();
        if (!active) return;
        final p = nodes || relay == null ? await repo.peers() : peers;
        if (!active) return;
        setState(() {
          relay = r;
          sessions = s;
          peers = p;
        });
      }
    } catch (e) {
      if (active) {
        setState(() {
          http = socks = relay = null;
          rows = [];
          sessions = [];
          peers = [];
        });
        fail(e);
      }
    } finally {
      refreshing = false;
    }
  }

  Future<void> act(Future<void> Function() action) async {
    if (!active || busy || refreshing) return;
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      final vault = VaultState.fromJson(
        await widget.api.request('/api/vault/status'),
      );
      if (!active) return;
      if (vault.hasError || vault.configured && !vault.unlocked) {
        widget.onLocked();
        return;
      }
      await action();
    } catch (e) {
      fail(e);
    } finally {
      if (active) setState(() => busy = false);
    }
    if (active) await refresh();
  }

  Widget button(String label, VoidCallback action, {bool enabled = true}) =>
      OutlinedButton(
        onPressed: busy || refreshing || !enabled ? null : action,
        child: Text(label),
      );
  Future<bool> confirm(String title) async =>
      await showDialog<bool>(
        context: context,
        builder: (c) => AlertDialog(
          title: Text(title),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(c, false),
              child: const Text('Отмена'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(c, true),
              child: const Text('Удалить'),
            ),
          ],
        ),
      ) ==
      true;
  void clearForm() {
    name.clear();
    id.clear();
    editing = null;
  }

  Future<void> proxyAction(
    String family,
    String action,
    String short,
  ) => act(() async {
    final result = await repo.proxyAction(
      family,
      action,
      action == 'start' ? repo.short(short) : short,
    );
    if (!active) return;
    // Only retain explicitly needed fields. Password is ephemeral, removed on wipe.
    setState(
      () => notice = action == 'stop'
          ? 'Демон удалил запись прокси. Закрытие слушающего сокета API не подтверждает.'
          : 'Демон принял запуск: ${result['listen_addr'] ?? result['local_port'] ?? 'адрес неизвестен'}${result['password'] is String ? ' · пользователь: ${result['username']} · пароль: ${result['password']}' : ''}',
    );
  });
  Widget proxyStatus(
    String title,
    Map<String, dynamic>? value,
    String family,
  ) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(
        '$title: ${value == null
            ? 'статус неизвестен'
            : value['active'] == true
            ? 'есть запись активного прокси'
            : 'активных записей нет'}',
      ),
      if (value != null)
        for (final p in (value['proxies'] as List).cast<Map<String, dynamic>>())
          Wrap(
            spacing: 8,
            children: [
              SelectableText(
                '${p['short_id']} · порт ${p['local_port'] ?? '—'} · ${p['started_at'] ?? ''}',
              ),
              button(
                'Стоп $title',
                () => proxyAction(family, 'stop', '${p['short_id']}'),
              ),
            ],
          ),
      if (value?['listen_addr'] != null)
        Text('Адрес: ${value!['listen_addr']}'),
    ],
  );
  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(widget.page, style: const TextStyle(fontSize: 22)),
      if (busy) const LinearProgressIndicator(),
      if (error != null)
        Text(error!, style: const TextStyle(color: Colors.red)),
      if (notice != null) SelectableText(notice!),
      button('Обновить', () => refresh(nodes: true)),
      if (gateways) ...[
        proxyStatus('HTTP', http, 'proxy'),
        proxyStatus('SOCKS5', socks, 'socks5'),
        const Text(
          'Stop меняет метаданные демона; закрытие сокета не подтверждено. Подключение/отключение узла в API — заглушка. Задержка: —.',
        ),
        TextField(
          controller: name,
          decoration: const InputDecoration(labelText: 'Имя шлюза'),
        ),
        TextField(
          controller: id,
          decoration: const InputDecoration(labelText: 'Short ID шлюза'),
        ),
        DropdownButton<String>(
          value: country,
          items: [
            for (final c in const ['RU', 'US', 'NL', 'DE', 'FR', 'GB'])
              DropdownMenuItem(value: c, child: Text(c)),
          ],
          onChanged: busy ? null : (v) => setState(() => country = v!),
        ),
        Wrap(
          spacing: 8,
          children: [
            button(
              editing == null ? 'Добавить шлюз' : 'Сохранить шлюз',
              () => act(() async {
                await repo.save(name.text, id.text, country, key: editing);
                if (active) setState(clearForm);
              }),
            ),
            button('Очистить форму', () => setState(clearForm)),
          ],
        ),
        for (final r in rows)
          Card(
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SelectableText(
                    '${r['name']} · ${r['short_id']} · ${r['country']}',
                  ),
                  Text(
                    'Соединение: ${r['connected'] == true ? 'есть' : 'нет'} · прокси по данным демона: ${r['proxy_active'] == true ? 'активен' : 'нет'} · задержка: —',
                  ),
                  Wrap(
                    spacing: 8,
                    children: [
                      button(
                        'Изменить',
                        () => setState(() {
                          editing = '${r['id']}';
                          name.text = '${r['name']}';
                          id.text = '${r['short_id']}';
                          country =
                              const [
                                'RU',
                                'US',
                                'NL',
                                'DE',
                                'FR',
                                'GB',
                              ].contains(r['country'])
                              ? '${r['country']}'
                              : 'RU';
                        }),
                      ),
                      button('Удалить', () async {
                        if (await confirm('Удалить шлюз ${r['name']}?') &&
                            active)
                          await act(() => repo.remove('${r['id']}'));
                      }),
                      for (final f in const ['proxy', 'socks5'])
                        button(
                          'Старт ${f == 'proxy' ? 'HTTP' : 'SOCKS5'}',
                          () => proxyAction(f, 'start', '${r['short_id']}'),
                          enabled:
                              http != null &&
                              socks != null &&
                              http!['active'] == false &&
                              socks!['active'] == false &&
                              r['connected'] == true,
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
      ] else ...[
        Text(
          'Сервер: ${relay == null
              ? 'статус неизвестен'
              : relay!['relay_server'] == true
              ? 'включён'
              : 'выключен'}',
        ),
        if (relay != null) ...[
          Text('Сессий: ${relay!['active_sessions']}'),
          Text(
            'Передано: ${relay!['total_bytes_forwarded']} Б · пакетов ${relay!['total_packets_forwarded']}',
          ),
        ],
        Wrap(
          spacing: 8,
          children: [
            button(
              'Запустить сервер',
              () => act(() => repo.server(true)),
              enabled: relay?['relay_server'] == false,
            ),
            button(
              'Остановить сервер',
              () => act(() => repo.server(false)),
              enabled: relay?['relay_server'] == true,
            ),
          ],
        ),
        const Text(
          'Запрос подключения не гарантирует публичную доступность Relay.',
        ),
        for (final s in sessions)
          Card(
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: SelectableText(
                '${s['session_id']} · ${s['source_peer']} → ${s['target_peer']}\n${s['status']} · ${s['bytes_forwarded']} Б · ${s['packets_forwarded']} пакетов · простой ${s['idle_time_secs']} с',
              ),
            ),
          ),
        for (final p in peers)
          Wrap(
            spacing: 8,
            children: [
              Text('${p['hostname'] ?? ''} · ${p['short_id']}'),
              button(
                'Подключить',
                () => act(() async {
                  await repo.connect('${p['short_id']}');
                  if (active)
                    setState(
                      () =>
                          notice = 'Запрос Relay отправлен. Проверьте сессии.',
                    );
                }),
              ),
            ],
          ),
      ],
    ],
  );
}
