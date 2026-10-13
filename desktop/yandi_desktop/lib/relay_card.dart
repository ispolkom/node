import 'dart:async';
import 'package:flutter/material.dart';
import 'preferences.dart';
import 'theme.dart';
import 'transport/api.dart';

/// Карточка «Ретрансляция» на «Главной»: узел сам служит ретранслятором по запросам других узлов и сам пользуется ретрансляторами,
/// если до него не достучаться. Здесь только состояние и одно слово-переключатель «служить ретранслятором для других».
class RelayCard extends StatefulWidget {
  final Api api;
  final bool Function() isActive;
  const RelayCard({super.key, required this.api, required this.isActive});

  @override
  State<RelayCard> createState() => _RelayCardState();
}

class _RelayCardState extends State<RelayCard> {
  Map<String, dynamic>? data;
  bool failed = false;
  bool busy = false;
  Timer? timer;

  @override
  void initState() {
    super.initState();
    load();
    timer = Timer.periodic(const Duration(seconds: 10), (_) => load());
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  Future<void> load() async {
    if (!mounted || !widget.isActive()) return;
    try {
      final r = await widget.api.request('/api/network/relay');
      if (mounted) {
        setState(() {
          data = r;
          failed = false;
        });
      }
    } catch (_) {
      if (mounted) setState(() => failed = true);
    }
  }

  Future<void> toggle(bool enabled) async {
    if (busy) return;
    setState(() => busy = true);
    try {
      await widget.api.request(
        '/api/network/relay',
        body: {'enabled': enabled},
        method: 'POST',
      );
      await load();
    } catch (_) {
      if (mounted) setState(() => failed = true);
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final d = data;
    final enabled = d?['relay_enabled'] == true;
    final client = d?['client'] == true;
    final fw = d?['forwarding'] is Map ? d!['forwarding'] as Map : null;
    return Panel(
      title: 'Ретрансляция',
      child: d == null
          ? UiText(failed ? 'Состояние недоступно.' : 'Загрузка…')
          : Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                UiText(
                  client
                      ? 'Этот узел недоступен снаружи и работает клиентом через ретрансляторы.'
                      : 'Этот узел доступен снаружи.',
                ),
                if (fw != null)
                  UiText(
                    'Сейчас пропускает: ${fw['active_sessions']} пар · передано ${DashboardBytes.fmt(fw['bytes'])}',
                  ),
                Wrap(
                  spacing: 10,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    UiText(
                      enabled
                          ? 'Служит ретранслятором для других по их запросу.'
                          : 'Ретранслятором для других не служит.',
                      style: const TextStyle(fontWeight: FontWeight.bold),
                    ),
                    InkWell(
                      key: const ValueKey('relay-toggle'),
                      onTap: busy ? null : () => toggle(!enabled),
                      child: UiText(
                        enabled ? 'Не служить' : 'Служить',
                        style: const TextStyle(
                          color: Icq.header,
                          decoration: TextDecoration.underline,
                        ),
                      ),
                    ),
                  ],
                ),
              ],
            ),
    );
  }
}

class DashboardBytes {
  static String fmt(dynamic bytes) {
    final v = bytes is num ? bytes.toDouble() : null;
    if (v == null) return '—';
    if (v < 1024) return '${v.toStringAsFixed(0)} Б';
    if (v < 1024 * 1024) return '${(v / 1024).toStringAsFixed(1)} КБ';
    return '${(v / 1024 / 1024).toStringAsFixed(1)} МБ';
  }
}
