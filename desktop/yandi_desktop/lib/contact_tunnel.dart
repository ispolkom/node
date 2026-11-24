import 'preferences.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'domain/shell_repository.dart';
import 'transport/api.dart';

class ContactTunnel extends StatefulWidget {
  final Api api;
  final String peer;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  const ContactTunnel({
    super.key,
    required this.api,
    required this.peer,
    required this.busy,
    required this.isActive,
    required this.mutate,
  });
  @override
  State<ContactTunnel> createState() => _ContactTunnelState();
}

class _ContactTunnelState extends State<ContactTunnel> {
  Timer? timer;
  bool loading = false;
  Map<String, dynamic>? tunnel;
  String? error, notice;
  bool get active => mounted && widget.isActive();
  String get peer => ShellRepository(widget.api).segment(widget.peer);
  @override
  void initState() {
    super.initState();
    check();
    timer = Timer.periodic(const Duration(seconds: 5), (_) {
      if (!widget.busy) check();
    });
  }

  @override
  void dispose() {
    timer?.cancel();
    tunnel = null;
    super.dispose();
  }

  Future<void> check() async {
    if (!active || loading) return;
    loading = true;
    try {
      final result = await widget.api.request('/api/tunnel/status/$peer');
      if (!active) return;
      if (result['tunnel'] is! Map<String, dynamic>)
        throw const ApiFailure('protocol');
      setState(() {
        tunnel = result['tunnel'] as Map<String, dynamic>;
        error = null;
      });
    } catch (e) {
      if (active)
        setState(() {
          tunnel = null;
          error = e is ApiFailure && e.kind == 'business'
              ? 'Нет подтверждённого туннеля: демон отклонил запрос статуса.'
              : e is ApiFailure
              ? e.message
              : 'Статус туннеля недоступен.';
        });
    } finally {
      loading = false;
    }
  }

  Future<void> command(bool start) async {
    await widget.mutate(() async {
      await widget.api.request(
        '/api/tunnel/${start ? 'start' : 'stop'}/$peer',
        method: 'POST',
        body: start ? {'type': 'generic'} : null,
      );
      if (!active) return;
      setState(() {
        notice = start
            ? 'Запрос туннеля отправлен. Готовность показывает статус демона.'
            : 'Команда остановки принята демоном.';
      });
      // Never leave a pre-command response looking current.
      while (loading && active) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      if (active) await check();
    });
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (tunnel != null)
        UiText(
          'Туннель: ${tunnel!['tunnel_status']} · ${tunnel!['tunnel_type']} · ↑${tunnel!['bytes_sent']} / ↓${tunnel!['bytes_received']} байт',
        ),
      if (error != null) UiText(error!),
      if (notice != null) UiText(notice!),
      Wrap(
        children: [
          TextButton(
            onPressed: widget.busy ? null : () => command(true),
            child: const UiText('Запустить туннель'),
          ),
          TextButton(
            onPressed: widget.busy ? null : () => command(false),
            child: const UiText('Остановить туннель'),
          ),
          TextButton(
            onPressed: widget.busy ? null : check,
            child: const UiText('Статус туннеля'),
          ),
        ],
      ),
    ],
  );
}
