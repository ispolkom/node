import 'theme.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'preferences.dart';
import 'transport/api.dart';
import 'domain/shell_repository.dart';
import 'groups.dart';

/// Only daemon-issued pairing material. No SVG engine, links or disk cache.
class DevicesPanel extends StatefulWidget {
  final Api api;
  final bool paired, busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  final Future<void> Function()? onChanged;
  const DevicesPanel({
    super.key,
    required this.api,
    required this.paired,
    required this.busy,
    required this.isActive,
    required this.mutate,
    this.onChanged,
  });
  @override
  State<DevicesPanel> createState() => _DevicesPanelState();
}

class _DevicesPanelState extends State<DevicesPanel> {
  List<Map<String, dynamic>> devices = [];
  String? text, error;
  QrSquares? qr;
  DateTime? expires;
  Timer? timer;
  int remaining = 0;
  bool loading = false;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    refresh();
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!active) {
        clear();
        return;
      }
      if (expires != null)
        setState(() {
          remaining--;
          if (remaining <= 0 || !DateTime.now().isBefore(expires!)) clear();
        });
      if (!widget.paired && DateTime.now().second % 10 == 0) refresh();
    });
  }

  void clear() {
    text = null;
    qr = null;
    expires = null;
  }

  @override
  void dispose() {
    timer?.cancel();
    clear();
    devices = [];
    super.dispose();
  }

  Future<void> refresh() async {
    if (!active || loading || widget.busy) return;
    loading = true;
    try {
      final result = await widget.api.request(
        widget.paired ? '/api/mobile/devices' : '/api/mobile/other-devices',
      );
      if (!active) return;
      final rows = ShellRepository.list(result, 'devices');
      if (rows.any(
        (d) => !RegExp(r'^[a-f0-9]{64}$').hasMatch('${d['peer_id']}'),
      ))
        throw const ApiFailure('protocol');
      setState(() {
        devices = rows;
        error = null;
      });
    } catch (e) {
      if (active)
        setState(() {
          devices = [];
          clear();
          error = e is ApiFailure
              ? e.message
              : const ApiFailure('protocol').message;
        });
    } finally {
      loading = false;
    }
  }

  /// [role]: `owner` — телефон владельца. Три роли сети: админ, пользователь, гость; пользователем человек становится, когда админ принимает его запрос гостя.
  Future<void> pairing([String role = 'owner']) async {
    clear();
    // Start TTL before request so network latency cannot prolong the secret.
    final started = DateTime.now();
    await widget.mutate(() async {
      final r = await widget.api.request(
        role == 'owner' ? '/api/mobile/pairing' : '/api/mobile/pairing?role=$role',
      );
      if (!active) return;
      final ttl = r['expires_in_secs'];
      if (ttl is! int || ttl <= 0 || ttl > 300 || r['qr_text'] is! String)
        throw const ApiFailure('protocol');
      final squares = QrSquares.parse('${r['qr_svg']}');
      final deadline = started.add(Duration(seconds: ttl));
      if (!DateTime.now().isBefore(deadline)) return;
      setState(() {
        text = r['qr_text'];
        qr = squares;
        expires = deadline;
        remaining = deadline.difference(DateTime.now()).inSeconds;
      });
    });
  }

  Future<void> action(Map<String, dynamic> d, String action) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => GroupConfirm(
        title: widget.paired
            ? 'Отозвать устройство?'
            : 'Изменить доступ устройства?',
        detail: widget.paired
            ? 'Токен устройства перестанет действовать.'
            : tr(context, actionLabels[action]!),
      ),
    );
    if (!active || ok != true) return;
    clear();
    await widget.mutate(() async {
      if (widget.paired) {
        final path = '/api/mobile/devices/${d['peer_id']}';
        try {
          await widget.api.request(path, method: 'DELETE');
        } on ApiFailure catch (e) {
          // устройство владеет группами: их удаление требует повторного ввода пароля узла
          if (e.status != 403) rethrow;
          if (!mounted) return;
          final value = await showDialog<List<String>>(
            context: context,
            builder: (_) => const GroupForm(
              title: 'Устройство владеет группами. Они будут удалены',
              labels: ['Пароль входа узла'],
              secret: true,
            ),
          );
          if (!active || value == null) return;
          await widget.api.request(
            path,
            method: 'DELETE',
            body: {'password': value[0]},
          );
        }
      } else {
        await widget.api.request(
          '/api/mobile/phone-action',
          body: {'peer': d['peer_id'], 'action': action},
        );
      }
      if (active) await widget.onChanged?.call();
    });
    if (active) await refresh();
  }

  static const actionLabels = {
    'add': 'Добавить устройство в контакты',
    'remove': 'Убрать устройство из контактов',
    'hide': 'Скрыть устройство',
    'unhide': 'Показать устройство',
    'block': 'Заблокировать устройство',
    'unblock': 'Разблокировать устройство',
  };
  @override
  Widget build(BuildContext context) => PanelStack(
    children: [
      UiText(
        widget.paired ? 'Сопряжённые устройства' : 'Другие устройства узла',
        style: const TextStyle(fontSize: 22),
      ),
      if (error != null) UiText(error!),
      if (loading) const LinearProgressIndicator(),
      TextButton(
        onPressed: widget.busy ? null : refresh,
        child: const UiText('Обновить устройства'),
      ),
      if (widget.paired)
        TextButton(
          onPressed: widget.busy ? null : pairing,
          child: const UiText('Создать код сопряжения'),
        ),
      if (text != null && expires != null) ...[
        if (qr != null)
          // Align снимает «растяжение» колонки: QR остаётся квадратом. Сторона — целое число экранных точек на единицу сетки (×2):
          // дробный масштаб давал модули разной ширины, и камера телефона не читала код с экрана.
          Align(
            alignment: Alignment.centerLeft,
            child: SizedBox(
              key: const ValueKey('pairing-qr'),
              width: (qr!.extent * 2).roundToDouble().clamp(280, 760),
              height: (qr!.extent * 2).roundToDouble().clamp(280, 760),
              child: CustomPaint(painter: qr),
            ),
          ),
        SelectableText(text!),
        UiText('Код действует ещё ${remaining.clamp(0, 300)} с'),
        TextButton(
          onPressed: () => setState(clear),
          child: const UiText('Скрыть код'),
        ),
      ],
      if (!loading && devices.isEmpty) const UiText('Устройств нет.'),
      for (final d in devices)
        Card(
          child: Padding(
            padding: const EdgeInsets.all(8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('${d['name']}'),
                SelectableText('${d['peer_id']}'),
                UiText(d['online'] == true ? 'В сети' : 'Не в сети'),
                if (widget.paired)
                  UiText(
                    'Соединений: ${d['connections']} · Последний контакт: ${d['last_seen_ms']}',
                  ),
                if (!widget.paired) UiText(stateLabel(context, d['status'])),
                Wrap(
                  children: [
                    for (final a
                        in widget.paired
                            ? ['revoke']
                            : [
                                if (d['status'] != 'blocked') 'add',
                                d['status'] == 'hidden' ? 'unhide' : 'hide',
                                d['status'] == 'blocked' ? 'unblock' : 'block',
                              ])
                      TextButton(
                        onPressed: widget.busy ? null : () => action(d, a),
                        child: UiText(
                          a == 'revoke'
                              ? 'Отозвать устройство'
                              : actionLabels[a]!,
                        ),
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

/// Accept only the qrcode Rust renderer's bounded rectangle grammar.
class QrSquares extends CustomPainter {
  final double extent;
  final List<Rect> squares;
  QrSquares(this.extent, this.squares);
  static QrSquares parse(String svg) {
    if (svg.length > 300000) throw const ApiFailure('protocol');
    final box = RegExp(r'viewBox="0 0 ([0-9]+) ([0-9]+)"').firstMatch(svg);
    final path = RegExp(
      r'<path fill="#[0-9a-fA-F]+" d="([^"]+)"/>',
    ).firstMatch(svg);
    if (box == null || path == null || box[1] != box[2])
      throw const ApiFailure('protocol');
    final n = double.parse(box[1]!);
    if (n < 21 || n > 4096) throw const ApiFailure('protocol');
    final data = path[1]!;
    final pattern = RegExp(
      r'M([0-9]+) ([0-9]+)h([0-9]+)v([0-9]+)H([0-9]+)V([0-9]+)',
    );
    final squares = <Rect>[];
    var end = 0;
    for (final m in pattern.allMatches(data)) {
      if (m.start != end || m[1] != m[5] || m[2] != m[6])
        throw const ApiFailure('protocol');
      final r = Rect.fromLTWH(
        double.parse(m[1]!),
        double.parse(m[2]!),
        double.parse(m[3]!),
        double.parse(m[4]!),
      );
      if (r.width <= 0 || r.height <= 0 || r.right > n || r.bottom > n)
        throw const ApiFailure('protocol');
      squares.add(r);
      end = m.end;
    }
    if (end != data.length || squares.isEmpty || squares.length > 40000)
      throw const ApiFailure('protocol');
    return QrSquares(n, squares);
  }

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = Colors.white);
    canvas.save();
    canvas.scale(size.width / extent, size.height / extent);
    final paint = Paint()
      ..color = Colors.black
      ..isAntiAlias = false;
    for (final r in squares) {
      canvas.drawRect(r, paint);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(QrSquares oldDelegate) => oldDelegate != this;
}
