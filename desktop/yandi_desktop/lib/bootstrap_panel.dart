import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'preferences.dart';
import 'theme.dart';
import 'transport/api.dart';
import 'domain/shell_repository.dart';

/// «Входной список узлов» в админке: ключ подписи списка (он только в закрытой среде) и заявки узлов с белым IP: данные подключения,
/// результат прозвона каждого адреса и готовая команда для списка. Заявка сама ничего не добавляет: решает владелец списка.
class BootstrapPanel extends StatefulWidget {
  final Api api;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const BootstrapPanel({super.key, required this.api, required this.busy, required this.isActive, required this.mutate});
  @override
  State<BootstrapPanel> createState() => _BootstrapPanelState();
}

class _BootstrapPanelState extends State<BootstrapPanel> {
  List<Map<String, dynamic>> apps = [];
  Map<String, dynamic>? key;
  String? error, copied;
  bool loading = false;
  Timer? _copiedTimer;
  bool get active => mounted && widget.isActive();

  @override
  void initState() {
    super.initState();
    refresh();
  }

  @override
  void dispose() {
    _copiedTimer?.cancel();
    super.dispose();
  }

  Future<void> refresh() async {
    if (!active || loading) return;
    loading = true;
    try {
      final a = await widget.api.request('/api/admin/bootstrap/applications');
      final k = await widget.api.request('/api/admin/bootstrap/pubkey');
      if (!active) return;
      final rows = ShellRepository.list(a, 'applications');
      setState(() {
        apps = rows;
        key = k;
        error = null;
      });
    } catch (e) {
      if (active) setState(() => error = e is ApiFailure ? e.message : const ApiFailure('protocol').message);
    } finally {
      loading = false;
    }
  }

  Future<void> copy(String what, String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (!active) return;
    setState(() => copied = what);
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => copied = null);
    });
  }

  Future<void> dismiss(Map<String, dynamic> a) async {
    await widget.mutate(() async {
      await widget.api.request('/api/admin/bootstrap/applications/dismiss', body: {'id': '${a['id']}'.substring(0, 16)});
    });
    if (active) await refresh();
  }

  static const statusLabels = {
    'new': 'Новая: адреса ещё прозваниваются',
    'verified': 'Проверена: адрес ответил ключом узла',
    'unreachable': 'Не отвечает по заявленным адресам',
    'dismissed': 'Отклонена',
    'listed': 'Уже в списке',
  };
  static const resultLabels = {'ok': 'отвечает', 'fail': 'не отвечает', 'skipped': 'не проверялся'};

  @override
  Widget build(BuildContext context) {
    final k = key;
    final pub = k?['pubkey'];
    return PanelStack(
      children: [
        const UiText('Входной список узлов', style: TextStyle(fontSize: 22)),
        const UiText('Ключ подписи списка хранится только в закрытой среде узла и никуда не передаётся; подписывает сам узел (yandi admin bootstrap sign).'),
        if (k != null)
          UiText(
            pub is String
                ? (k['pinned'] == true ? 'Открытый ключ подписи: $pub' : 'Открытый ключ подписи: $pub (ещё не закреплён в программе)')
                : 'Закрытая среда заперта: ключ подписи недоступен.',
            key: const ValueKey('bootstrap-key'),
          ),
        if (pub is String)
          TextButton(
            key: const ValueKey('bootstrap-copy-key'),
            onPressed: () => copy('key', pub),
            child: UiText(copied == 'key' ? 'Скопировано' : 'Скопировать открытый ключ'),
          ),
        if (error != null) UiText(error!),
        const UiText('Заявки узлов', style: TextStyle(fontSize: 18)),
        const UiText('Узлы с белым IP сами присылают данные подключения тем, кто в списке. Заявка ничего не добавляет: решаете вы.'),
        TextButton(onPressed: widget.busy ? null : refresh, child: const UiText('Обновить заявки')),
        if (loading) const LinearProgressIndicator(),
        if (!loading && apps.isEmpty) const UiText('Заявок нет.', key: ValueKey('bootstrap-empty')),
        for (final a in apps)
          Card(
            key: ValueKey('app-${a['id']}'),
            child: Padding(
              padding: const EdgeInsets.all(8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SelectableText('${a['id']}', style: const TextStyle(fontSize: 12)),
                  UiText(statusLabels[a['status']] ?? 'Неизвестный статус'),
                  if (a['region'] is String) UiText('Страна: ${a['region']}', style: const TextStyle(fontSize: 12)),
                  UiText('Роли: ${(a['roles'] as List?)?.join(', ') ?? ''}'),
                  for (final p in (a['probes'] as List?) ?? const [])
                    UiText('${p['kind'] == 'mobile' ? 'Телефон' : 'Узел'} ${p['addr']}: ${resultLabels[p['result']] ?? 'не проверялся'}', style: const TextStyle(fontSize: 12)),
                  if (((a['probes'] as List?) ?? const []).isEmpty)
                    for (final addr in [...(a['addr'] as List? ?? const []), ...(a['mobile'] as List? ?? const [])]) UiText('$addr: ещё не проверялся', style: const TextStyle(fontSize: 12)),
                  Wrap(
                    children: [
                      TextButton(
                        key: ValueKey('app-copy-${a['id']}'),
                        onPressed: () => copy('${a['id']}', '${a['command']}'),
                        child: UiText(copied == '${a['id']}' ? 'Скопировано' : 'Скопировать команду для списка'),
                      ),
                      if (a['status'] != 'dismissed' && a['status'] != 'listed')
                        TextButton(
                          key: ValueKey('app-dismiss-${a['id']}'),
                          onPressed: widget.busy ? null : () => dismiss(a),
                          child: const UiText('Отклонить'),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}
