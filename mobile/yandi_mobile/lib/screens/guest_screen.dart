import 'dart:math';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../l10n.dart';
import '../models/guest_node.dart';
import '../models/trusted_node.dart';
import '../services/app_state.dart';
import '../services/bootstrap_loader.dart';
import '../services/guest_pair.dart';
import '../services/node_hello.dart';
import '../theme.dart';
import 'pair_screen.dart';

/// Вкладка «Гость» первого экрана: предупреждение об ограничениях и ОДНА кнопка входа. Какой узел выбран, человеку не показываем
/// (ни адрес, ни регион, ни название): приложение само берёт узлы из подписанного списка в случайном порядке и пробует по очереди.
class GuestPanel extends StatefulWidget {
  /// Для тестов: готовый список узлов, вход и переход после входа.
  final List<GuestNode>? nodes;
  final Future<TrustedNode> Function(GuestNode)? enter;
  final void Function(BuildContext)? onEntered;
  final BootstrapLoader? loader;
  final void Function(bool busy)? busyChanged;
  const GuestPanel({super.key, this.nodes, this.enter, this.onEntered, this.loader, this.busyChanged});
  @override
  State<GuestPanel> createState() => _GuestPanelState();
}

class _GuestPanelState extends State<GuestPanel> {
  List<GuestNode>? _nodes;
  String? _error;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    if (widget.nodes != null) {
      _nodes = widget.nodes;
    } else {
      // подписанный список входных узлов: сеть, копия на телефоне, вшитая копия (проверка подписи в BootstrapLoader)
      (widget.loader ?? BootstrapLoader()).load().then((r) {
        if (mounted) setState(() => _nodes = r?.doc.guestNodes() ?? const []);
      }).catchError((_) {
        if (mounted) setState(() => _nodes = const []);
      });
    }
  }

  void _setBusy(bool b) {
    if (mounted) setState(() => _busy = b);
    widget.busyChanged?.call(b);
  }

  Future<void> _enter() async {
    final all = _nodes;
    if (_busy || all == null || all.isEmpty) return;
    _setBusy(true);
    setState(() => _error = null);
    final order = List<GuestNode>.of(all)..shuffle(Random.secure());
    String? last;
    try {
      for (final n in order) {
        try {
          final node = widget.enter != null ? await widget.enter!(n) : await _realEnter(n);
          if (!mounted) return;
          if (widget.onEntered == null) await context.read<AppState>().completePairing(node);
          if (!mounted) return;
          if (widget.onEntered != null) {
            widget.onEntered!(context);
          } else {
            Navigator.of(context).pushNamedAndRemoveUntil('/home', (_) => false);
          }
          return;
        } on GuestPairError catch (e) {
          last = e.message;
        } catch (_) {
          last = tr('Не удалось войти гостем. Попробуйте ещё раз.');
        }
      }
      if (mounted) setState(() => _error = last ?? tr('Не удалось войти гостем. Попробуйте ещё раз.'));
    } finally {
      _setBusy(false);
    }
  }

  /// Узел из подписанного списка сначала представляется (подпись его сертификата ключом из списка), и только с проверенным отпечатком идёт вход.
  Future<TrustedNode> _realEnter(GuestNode n) async {
    final verified = n.key.isNotEmpty ? n.withFingerprint(await verifyNodeHello(n)) : n;
    if (verified.fingerprint.isEmpty) throw GuestPairError(tr('Не удалось проверить узел: вход гостем отменён.'));
    return guestPair(verified, client: buildPinnedPairClient(verified.fingerprint), deviceName: newDeviceLabel());
  }

  @override
  Widget build(BuildContext context) {
    final nodes = _nodes;
    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        Text(tr('Вы подключаетесь к частной сети YANDI'), key: const ValueKey('pair-title'),
            style: const TextStyle(color: AppTheme.text, fontSize: 18, fontWeight: FontWeight.bold)),
        const SizedBox(height: 10),
        Container(
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(color: AppTheme.surface, borderRadius: BorderRadius.circular(10), border: Border.all(color: Colors.orangeAccent.withValues(alpha: 0.6))),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Row(children: [
              const Icon(Icons.warning_amber_rounded, color: Colors.orangeAccent, size: 20),
              const SizedBox(width: 8),
              Text(tr('Внимание'), style: const TextStyle(color: Colors.orangeAccent, fontWeight: FontWeight.bold)),
            ]),
            const SizedBox(height: 8),
            Text(tr('Если у вас нет кода или ссылки, вы можете войти с ограниченными возможностями и написать администратору узла запрос на статус пользователя в разделе «Настройки». Администрация узла принимает решение на своё усмотрение. После входа гостем можно сменить узел, если есть другие активные.'),
                key: const ValueKey('guest-intro'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 14, height: 1.35)),
            const SizedBox(height: 10),
            Text(tr('Гостю можно: переписка с участниками и компьютером узла (до 10 сообщений в сутки), голосовые сообщения, аудио и видео в чате, аудиозвонки (до 15 минут, до 60 минут в сутки), жалобы. Нельзя: группы, видеозвонки, выход в интернет через узел, файлы больше 30 МБ, больше 20 файлов или 300 МБ в сутки. Гость живёт на узле сутки, затем стирается; три жалобы от разных людей — гость удаляется.'),
                key: const ValueKey('guest-limits'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13, height: 1.35)),
          ]),
        ),
        const SizedBox(height: 16),
        if (_error != null)
          Padding(padding: const EdgeInsets.only(bottom: 12), child: Text(_error!, key: const ValueKey('guest-error'), style: const TextStyle(color: Colors.redAccent))),
        if (nodes == null)
          const Center(child: CircularProgressIndicator(color: AppTheme.accent))
        else if (nodes.isEmpty)
          Text(tr('Сейчас нет узлов, принимающих гостей. Попросите код или ссылку у администратора узла.'), key: const ValueKey('guest-empty'), style: const TextStyle(color: Colors.orangeAccent))
        else
          FilledButton(
            key: const ValueKey('guest-enter'),
            onPressed: _busy ? null : _enter,
            child: Text(_busy ? tr('Входим…') : tr('Войти как гость')),
          ),
        const SizedBox(height: 12),
        Text(tr('Есть QR, код или ссылка? Откройте вкладку «QR» или «Код» внизу.'), key: const ValueKey('guest-hint'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13)),
      ],
    );
  }
}
