import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import '../services/share_text.dart';
import '../theme.dart';

/// Код приглашения нового устройства: QR и текст (можно переслать).
typedef Invite = ({String text, String link, String code, String pin, int ttlSecs});
typedef InviteMaker = Future<Invite> Function();

class InviteDeviceScreen extends StatefulWidget {
  final InviteMaker create;
  final Future<void> Function() revoke;

  /// Ещё живое приглашение (после закрытия и повторного открытия приложения); null — нет.
  final Future<Invite?> Function()? restore;
  const InviteDeviceScreen({super.key, required this.create, required this.revoke, this.restore});
  @override
  State<InviteDeviceScreen> createState() => _InviteDeviceScreenState();
}

String _left(int s) {
  final v = s.clamp(0, 99999);
  return v >= 60 ? '${v ~/ 60} мин ${v % 60} с' : '$v с';
}

class _InviteDeviceScreenState extends State<InviteDeviceScreen> {
  String? text;
  String? code;
  String? pin;
  String? link;
  String? error;
  int remaining = 0;
  bool busy = false;
  Timer? timer;
  bool _dialogOpen = false;

  @override
  void initState() {
    super.initState();
    widget.restore?.call().then((inv) {
      if (inv != null && mounted && text == null) {
        setState(() {
          text = inv.text;
          link = inv.link;
          code = inv.code;
          pin = inv.pin;
          remaining = inv.ttlSecs;
        });
      }
    }).catchError((_) {});
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (text == null || !mounted) return;
      setState(() {
        remaining--;
        if (remaining <= 0) {
          _clear();
          _closePinDialog();
        }
      });
    });
  }

  /// Окно с PIN не переживает приглашение: истекло или отозвано — окно закрывается, чтобы не показывать и не копировать уже мёртвый PIN.
  void _closePinDialog() {
    if (_dialogOpen && mounted) {
      _dialogOpen = false;
      Navigator.of(context).pop();
    }
  }

  void _clear() {
    text = null;
    code = null;
    pin = null;
    link = null;
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
        link = inv.link;
        code = inv.code;
        pin = inv.pin;
        remaining = inv.ttlSecs;
      });
    } catch (e) {
      if (mounted) setState(() => error = e is StateError ? e.message : 'Не удалось получить код приглашения');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  /// После копирования или отправки напоминаем PIN: его называют приглашённому другим каналом, он в ссылку не входит.
  Future<void> _pinReminder(String what) async {
    final shown = pin;
    if (!mounted || shown == null) return;
    _dialogOpen = true;
    await showDialog<void>(
      context: context,
      builder: (c) => AlertDialog(
        key: const ValueKey('invite-pin-dialog'),
        title: Text(what),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              'Внимание! Не отправляйте ссылку-приглашение и PIN в одно место. Лучше продиктуйте PIN по телефону или вышлите по почте, в SMS или в другом мессенджере. '
              'Злоумышленники могут воспользоваться ссылкой вместе с PIN, и тогда приглашённый получит ссылку, которая уже не будет работать.',
              key: ValueKey('invite-pin-warning'),
            ),
            const SizedBox(height: 12),
            Text(shown, style: const TextStyle(fontSize: 36, letterSpacing: 8, fontWeight: FontWeight.bold)),
          ],
        ),
        actions: [
          TextButton(onPressed: () => Clipboard.setData(ClipboardData(text: shown)), child: const Text('Копировать PIN')),
          TextButton(onPressed: () { _dialogOpen = false; Navigator.of(c).pop(); }, child: const Text('Понятно')),
        ],
      ),
    );
    _dialogOpen = false;
  }

  Future<void> _revoke() async {
    try {
      await widget.revoke();
    } catch (e) {
      // узел отзыв не принял: код продолжает действовать, поэтому с экрана он не убирается, а ошибка видна
      if (mounted) setState(() => error = e is StateError ? e.message : 'Не удалось отозвать код: нет связи с узлом. Код ещё действует, повторите.');
      return;
    }
    if (!mounted) return;
    _closePinDialog();
    setState(() {
      error = null;
      _clear();
    });
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
            'Покажите QR или перешлите ссылку тому, кого приглашаете: он подключится к этому же узлу. Ссылка одноразовая и живёт час. '
            'Чтобы её не мог использовать чужой, приглашённому понадобится PIN из 4 цифр: назовите его другим способом (голосом, в другом чате). Приглашённый получит права обычного пользователя узла, но сам приглашать не сможет, пока администратор узла '
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
            Text('Приглашение действует ещё ${_left(remaining)}', key: const ValueKey('invite-timer'), style: const TextStyle(color: AppTheme.textSecondary)),
            const SizedBox(height: 8),
            Text('PIN: $pin', key: const ValueKey('invite-pin'), style: const TextStyle(color: AppTheme.text, fontSize: 28, letterSpacing: 6, fontWeight: FontWeight.bold)),
            const Text('Назовите PIN приглашённому другим каналом, не вместе со ссылкой.', style: TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            const SizedBox(height: 8),
            SelectableText('Код: $code', key: const ValueKey('invite-code'), style: const TextStyle(color: AppTheme.text, fontSize: 20, letterSpacing: 2)),
            const SizedBox(height: 8),
            Wrap(
              children: [
                TextButton.icon(
                  key: const ValueKey('invite-share'),
                  onPressed: () async {
                    final ok = await shareText(link!);
                    if (!ok) await Clipboard.setData(ClipboardData(text: link!));
                    await _pinReminder(ok ? 'Ссылка отправлена.' : 'Ссылка скопирована.');
                  },
                  icon: const Icon(Icons.share, size: 18),
                  label: const Text('Отправить ссылку'),
                ),
                TextButton.icon(
                  key: const ValueKey('invite-copy'),
                  onPressed: () async {
                    await Clipboard.setData(ClipboardData(text: text!));
                    await _pinReminder('Текст приглашения скопирован.');
                  },
                  icon: const Icon(Icons.copy, size: 18),
                  label: const Text('Скопировать текст'),
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
