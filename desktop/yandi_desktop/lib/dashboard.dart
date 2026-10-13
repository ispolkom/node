import 'package:flutter/material.dart';
import 'preferences.dart';
import 'theme.dart';

/// «Главная»: отдельные карточки, как в веб-интерфейсе узла — статус, управление, обнаруженные узлы и плитки метрик.
class DashboardView extends StatelessWidget {
  final Map<String, dynamic>? status, speed;
  final List<dynamic> nodes;
  final Widget controls;
  const DashboardView({
    super.key,
    required this.status,
    required this.speed,
    required this.nodes,
    required this.controls,
  });

  static String rate(dynamic bytesPerSecond) {
    final v = bytesPerSecond is num ? bytesPerSecond.toDouble() : null;
    if (v == null) return '—';
    if (v < 1024) return '${v.toStringAsFixed(0)} Б/с';
    if (v < 1024 * 1024) return '${(v / 1024).toStringAsFixed(1)} КБ/с';
    return '${(v / 1024 / 1024).toStringAsFixed(2)} МБ/с';
  }

  static String size(dynamic bytes) {
    final v = bytes is num ? bytes.toDouble() : null;
    if (v == null) return '—';
    if (v < 1024) return '${v.toStringAsFixed(0)} Б';
    if (v < 1024 * 1024) return '${(v / 1024).toStringAsFixed(1)} КБ';
    return '${(v / 1024 / 1024).toStringAsFixed(1)} МБ';
  }

  static String percent(dynamic v) => v is num ? '${v.toStringAsFixed(1)}%' : '—';

  @override
  Widget build(BuildContext context) {
    const gap = SizedBox(height: 12);
    final online = status?['status'] == 'online';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Panel(
          title: 'Статус узла',
          trailing: status == null
              ? null
              : _Badge(
                  online ? 'Online' : 'Offline',
                  online ? const Color(0xFF2E7D32) : const Color(0xFFC62828),
                ),
          child: status == null
              ? const UiText('Загрузка статуса…')
              : Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    for (final item in const {
                      'short_id': 'Short ID',
                      'cid': 'CID',
                      'role': 'Роль',
                      'is_local': 'Локальный узел',
                      'external_ip': 'Внешний IP',
                      'virtual_ipv6': 'Виртуальная IPv6',
                      'ipv6_short': 'Короткая IPv6',
                      'nat_status': 'NAT',
                      'mode': 'Режим',
                      'discovery_port': 'Порт обнаружения',
                      'data_port': 'Порт данных',
                      'web_port': 'Веб-порт',
                      'active_proxies': 'Активные прокси',
                    }.entries)
                      _Tile(item.value, status![item.key] ?? '—'),
                    _Tile('ID узла', status!['node_id'] ?? '—', wide: true),
                  ],
                ),
        ),
        gap,
        Panel(title: 'Управление', child: controls),
        gap,
        Panel(
          title: 'Обнаруженные узлы',
          trailing: _Badge('${nodes.length}', Icq.light, dark: true),
          child: nodes.isEmpty
              ? const UiText('Обнаруженных узлов нет.')
              : Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    for (final node in nodes.whereType<Map>())
                      _NodeCard(Map<String, dynamic>.from(node)),
                  ],
                ),
        ),
        gap,
        Wrap(
          spacing: 12,
          runSpacing: 12,
          children: [
            _StatCard(
              icon: Icons.north_east,
              label: 'Отправка',
              value: speed == null ? '—' : rate(speed!['tx_speed']),
              lines: speed == null
                  ? const []
                  : [
                      'Оценка приёма соседа: ${rate(speed!['peer_rx_estimate'])}',
                      'Path0 соседа: ${percent(speed!['peer_path0_loss'])}',
                    ],
            ),
            _StatCard(
              icon: Icons.south_west,
              label: 'Приём',
              value: speed == null ? '—' : rate(speed!['rx_speed']),
              lines: speed == null
                  ? const []
                  : [
                      'Path0, потери: ${percent(speed!['path0_loss_incoming'])}',
                      'Попадания клонов: ${percent(speed!['clone_hit_pct'])}',
                    ],
            ),
            _StatCard(
              icon: Icons.bolt,
              label: 'Пинг',
              value: speed == null || speed!['avg_rtt_ms'] is! num
                  ? '—'
                  : '${(speed!['avg_rtt_ms'] as num).toStringAsFixed(0)} мс',
              lines: speed == null
                  ? const []
                  : [
                      'Соседей: ${nodes.length}',
                      'Отброшено + CRC: ${percent(speed!['wagon_drop_crc_pct'])}',
                    ],
            ),
            _StatCard(
              icon: Icons.train,
              label: 'Транспорт',
              value: speed == null ? '—' : 'Депо: ${size(speed!['depot_bytes'])}',
              lines: speed == null
                  ? const []
                  : [
                      'Активные поезда: ${speed!['active_trains'] ?? '—'}',
                      'Вытеснения: ${speed!['evictions_delta'] ?? '—'} / ${speed!['evictions_total'] ?? '—'}',
                    ],
            ),
          ],
        ),
      ],
    );
  }
}

class _Badge extends StatelessWidget {
  final String text;
  final Color color;
  final bool dark;
  const _Badge(this.text, this.color, {this.dark = false});
  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 3),
    decoration: BoxDecoration(color: color),
    child: Text(
      text,
      style: TextStyle(
        fontSize: 12,
        fontWeight: FontWeight.bold,
        color: dark ? Icq.text : Colors.white,
      ),
    ),
  );
}

/// Плитка «подпись — значение» из карточки статуса.
class _Tile extends StatelessWidget {
  final String label;
  final dynamic value;
  final bool wide;
  const _Tile(this.label, this.value, {this.wide = false});
  @override
  Widget build(BuildContext context) => Container(
    width: wide ? 440 : 200,
    padding: const EdgeInsets.all(10),
    decoration: BoxDecoration(
      color: Colors.white,
      border: Border.all(color: Icq.border),
    ),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        UiText(label, style: const TextStyle(fontSize: 11, color: Icq.dark)),
        const SizedBox(height: 3),
        SelectableText(
          '$value',
          style: const TextStyle(fontSize: 14, fontWeight: FontWeight.bold),
        ),
      ],
    ),
  );
}

class _NodeCard extends StatelessWidget {
  final Map<String, dynamic> node;
  const _NodeCard(this.node);
  @override
  Widget build(BuildContext context) {
    final connected = node['connected'] == true;
    return Container(
      width: 300,
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: Colors.white,
        border: Border.all(color: connected ? Icq.header : Icq.border, width: connected ? 2 : 1),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '${node['hostname'] ?? node['short_id'] ?? '—'}',
            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.bold),
          ),
          for (final e in const {
            'short_id': 'Short ID',
            'source': 'Источник',
            'role': 'Роль',
            'nat_status': 'NAT',
            'connected': 'Соединение',
            'addresses': 'Адреса',
            'admin_port': 'Порт управления',
            'discovery_port': 'Порт обнаружения',
            'data_port': 'Порт данных',
          }.entries)
            if (node.containsKey(e.key))
              SelectableText(
                '${tr(context, e.value)}: ${node[e.key]}',
                style: const TextStyle(fontSize: 12, color: Icq.dark),
              ),
        ],
      ),
    );
  }
}

class _StatCard extends StatelessWidget {
  final IconData icon;
  final String label, value;
  final List<String> lines;
  const _StatCard({
    required this.icon,
    required this.label,
    required this.value,
    required this.lines,
  });
  @override
  Widget build(BuildContext context) => Container(
    width: 270,
    padding: const EdgeInsets.all(14),
    decoration: const BoxDecoration(color: Icq.panel, border: Icq.frame),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(icon, size: 34, color: Icq.header),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              UiText(label, style: const TextStyle(fontSize: 12, color: Icq.dark)),
              Text(
                value,
                style: const TextStyle(fontSize: 20, fontWeight: FontWeight.bold, color: Icq.header),
              ),
              for (final l in lines)
                UiText(l, style: const TextStyle(fontSize: 11, color: Icq.dark)),
            ],
          ),
        ),
      ],
    ),
  );
}
