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

  /// Число колонок равных карточек по ширине блока: карточки занимают всю ширину содержимого и не растягиваются от текста.
  static Widget grid(double width, List<Widget> cards, {required double height, int maxColumns = 3, double gap = 10}) {
    final columns = width >= 880 && maxColumns >= 3 ? maxColumns : width >= 580 && maxColumns >= 2 ? (maxColumns >= 4 && width >= 980 ? maxColumns : 2) : 1;
    final cell = (width - gap * (columns - 1)) / columns;
    return Wrap(
      spacing: gap,
      runSpacing: gap,
      children: [for (final c in cards) SizedBox(width: cell, height: height, child: c)],
    );
  }

  @override
  Widget build(BuildContext context) {
    const gap = SizedBox(height: 12);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Panel(title: 'Управление', child: controls),
        gap,
        Panel(
          title: 'Обнаруженные узлы',
          trailing: _Badge('${nodes.length}', Icq.light, dark: true),
          child: nodes.isEmpty
              ? const UiText('Обнаруженных узлов нет.')
              : LayoutBuilder(
                  builder: (context, c) => grid(
                    c.maxWidth,
                    [for (final node in nodes.whereType<Map>()) _NodeCard(Map<String, dynamic>.from(node))],
                    height: 196,
                  ),
                ),
        ),
        gap,
        LayoutBuilder(
          builder: (context, c) => grid(
            c.maxWidth,
            [
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
            height: 112,
            maxColumns: 4,
            gap: 12,
          ),
        ),
      ],
    );
  }
}

/// Вкладка «Статус узла»: каждая строка — отдельный белый блок одинаковой ширины и высоты; подпись слева, значение справа.
class NodeStatusView extends StatelessWidget {
  final Map<String, dynamic>? status;
  const NodeStatusView({super.key, required this.status});

  static const rows = {
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
    'node_id': 'ID узла',
  };

  /// Внешние прокси из настроек: «работают из добавлено» по последней проверке (раз в сутки); пока среда заперта — прочерк.
  static String upstream(Map<String, dynamic> st) {
    final added = st['upstream_added'], working = st['upstream_working'];
    if (added is! int || working is! int) return '—';
    return '$working из $added';
  }

  /// Значение для показа: логические — Да/Нет; «активные прокси» — только есть или нет, без самих прокси.
  static String show(String key, dynamic v) {
    if (key == 'active_proxies') {
      final has = v is num ? v > 0 : v is Iterable ? v.isNotEmpty : v is Map ? v.isNotEmpty : v is bool ? v : v is String && v.isNotEmpty && v != '0';
      return has ? 'Да' : 'Нет';
    }
    if (v is bool) return v ? 'Да' : 'Нет';
    if (v == null || (v is String && v.isEmpty)) return '—';
    return '$v';
  }

  @override
  Widget build(BuildContext context) {
    final online = status?['status'] == 'online';
    return Panel(
      title: 'Статус узла',
      trailing: status == null
          ? null
          : _Badge(
              online ? 'Online' : 'Offline',
              online ? const Color(0xFF2E7D32) : const Color(0xFFC62828),
            ),
      child: status == null
          ? const UiText('Загрузка статуса…')
          : SelectionArea(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final item in rows.entries)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 6),
                      child: _StatusRow(item.value, item.key == 'active_proxies' ? upstream(status!) : show(item.key, status![item.key])),
                    ),
                ],
              ),
            ),
    );
  }
}

class _StatusRow extends StatelessWidget {
  final String label, value;
  const _StatusRow(this.label, this.value);
  @override
  Widget build(BuildContext context) => Container(
    height: 40,
    padding: const EdgeInsets.symmetric(horizontal: 12),
    decoration: BoxDecoration(color: Colors.white, border: Border.all(color: Icq.border)),
    child: Row(
      children: [
        SizedBox(
          width: 190,
          child: UiText(label, style: const TextStyle(fontSize: 13, color: Icq.dark)),
        ),
        Expanded(
          child: Align(
            alignment: Alignment.centerRight,
            child: FittedBox(
              fit: BoxFit.scaleDown,
              alignment: Alignment.centerRight,
              child: Text(value, maxLines: 1, style: const TextStyle(fontSize: 14, fontWeight: FontWeight.bold, fontStyle: FontStyle.italic)),
            ),
          ),
        ),
      ],
    ),
  );
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

class _NodeCard extends StatelessWidget {
  final Map<String, dynamic> node;
  const _NodeCard(this.node);
  @override
  Widget build(BuildContext context) {
    final connected = node['connected'] == true;
    return Container(
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
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
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
              Text(
                '${tr(context, e.value)}: ${node[e.key] is bool ? (node[e.key] ? 'Да' : 'Нет') : node[e.key]}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
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
