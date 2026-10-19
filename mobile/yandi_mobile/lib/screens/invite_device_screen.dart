import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import '../theme.dart';

/// Код приглашения нового устройства: QR и текст (можно переслать).
typedef InviteMaker = Future<({String text, String code, int ttlSecs})> Function();

class InviteDeviceScreen extends StatefulWidget {
  final InviteMaker create;
  final Future<void> Function() revoke;
  const InviteDeviceScreen({super.key, required this.create, required this.revoke});
  @override
  State<InviteDeviceScreen> createState() => _InviteDeviceScreenState();
}

class _InviteDeviceScreenState extends State<InviteDeviceScreen> {
  String? text;
  String? code;
  String? error;
  int remaining = 0;
  bool busy = false;
  Timer? timer;

  @override
  void initState() {
    super.initState();
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (text == null || !mounted) return;
      setState(() {
        remaining--;
        if (remaining <= 0) _clear();
      });
    });
  }

  void _clear() {
    text = null;
    code = null;
  }

  @override
  void dispose() {
    timer?.cancel();
    _clear();
    super.dispose();
  }

  Future<void> _make() async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
      _clear();
    });
    try {
      final inv = await widget.create();
      if (!mounted) return;
      setState(() {
        text = inv.text;
        code = inv.code;
        remaining = inv.ttlSecs;
      });
    } catch (e) {
      if (mounted) setState(() => error = e is StateError ? e.message : 'Не удалось получить код приглашения');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> _revoke() async {
    try {
      await widget.revoke();
    } catch (_) {}
    if (mounted) setState(_clear);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: const Text('Пригласить устройство', style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Text(
            'Покажите QR или перешлите текстовый код тому, кого приглашаете: он подключится к этому же узлу. Код одноразовый и живёт '
            'несколько минут. Приглашённый получит права обычного пользователя узла, но сам приглашать не сможет, пока администратор узла '
            'этого не разрешит. Администратор видит, кто кого пригласил.',
            style: TextStyle(color: AppTheme.textSecondary, fontSize: 13),
          ),
          const SizedBox(height: 16),
          if (error != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: Text(error!, key: const ValueKey('invite-error'), style: const TextStyle(color: Colors.redAccent)),
            ),
          if (text == null)
            FilledButton(
              key: const ValueKey('invite-make'),
              onPressed: busy ? null : _make,
              child: Text(busy ? 'Готовлю код…' : 'Получить код приглашения'),
            )
          else ...[
            Center(
              child: Container(
                color: Colors.white,
                padding: const EdgeInsets.all(12),
                child: QrImageView(key: const ValueKey('invite-qr'), data: text!, size: 260, backgroundColor: Colors.white),
              ),
            ),
            const SizedBox(height: 12),
            Text('Код действует ещё ${remaining.clamp(0, 99999)} с', key: const ValueKey('invite-timer'), style: const TextStyle(color: AppTheme.textSecondary)),
            const SizedBox(height: 8),
            SelectableText('Код: $code', key: const ValueKey('invite-code'), style: const TextStyle(color: AppTheme.text, fontSize: 20, letterSpacing: 2)),
            const SizedBox(height: 8),
            Row(
              children: [
                TextButton.icon(
                  key: const ValueKey('invite-copy'),
                  onPressed: () => Clipboard.setData(ClipboardData(text: text!)),
                  icon: const Icon(Icons.copy, size: 18),
                  label: const Text('Скопировать текст приглашения'),
                ),
              ],
            ),
            TextButton(key: const ValueKey('invite-revoke'), onPressed: _revoke, child: const Text('Отозвать код')),
          ],
        ],
      ),
    );
  }
}
