import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show rootBundle;
import 'package:http/http.dart' as http;
import 'package:provider/provider.dart';
import '../l10n.dart';
import '../models/guest_node.dart';
import '../models/trusted_node.dart';
import '../services/app_state.dart';
import '../services/guest_pair.dart';
import '../theme.dart';
import 'pair_screen.dart';

/// Вход гостем: без кода и ссылки, с ограничениями. Узлы берутся из вшитого списка (его заменяет подписанный выпуск).
class GuestScreen extends StatefulWidget {
  /// Для тестов: готовый список узлов, вход и переход после входа.
  final List<GuestNode>? nodes;
  final Future<TrustedNode> Function(GuestNode)? enter;
  final void Function(BuildContext)? onEntered;
  const GuestScreen({super.key, this.nodes, this.enter, this.onEntered});
  @override
  State<GuestScreen> createState() => _GuestScreenState();
}

class _GuestScreenState extends State<GuestScreen> {
  List<GuestNode>? _nodes;
  String? _error;
  GuestNode? _busy;

  @override
  void initState() {
    super.initState();
    if (widget.nodes != null) {
      _nodes = widget.nodes;
    } else {
      rootBundle.loadString('assets/guest_nodes.json', cache: false).then((j) {
        if (mounted) setState(() => _nodes = GuestNode.parseList(j));
      }).catchError((_) {
        if (mounted) setState(() => _nodes = const []);
      });
    }
  }

  Future<void> _enter(GuestNode n) async {
    if (_busy != null) return;
    setState(() {
      _busy = n;
      _error = null;
    });
    try {
      final node = widget.enter != null
          ? await widget.enter!(n)
          : await guestPair(n, client: buildPinnedPairClient(n.fingerprint), deviceName: newDeviceLabel());
      if (!mounted) return;
      if (widget.onEntered == null) await context.read<AppState>().completePairing(node);
      if (!mounted) return;
      if (widget.onEntered != null) {
        widget.onEntered!(context);
      } else {
        Navigator.of(context).pushNamedAndRemoveUntil('/home', (_) => false);
      }
    } on GuestPairError catch (e) {
      if (mounted) setState(() => _error = e.message);
    } catch (_) {
      if (mounted) setState(() => _error = tr('Не удалось войти гостем. Попробуйте ещё раз.'));
    } finally {
      if (mounted) setState(() => _busy = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final nodes = _nodes;
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        iconTheme: const IconThemeData(color: AppTheme.text),
        title: Text(tr('Вход гостем'), style: const TextStyle(color: AppTheme.text)),
        actions: const [LangToggle(), SizedBox(width: 8)],
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Text(tr('Вы подключаетесь к частной сети YANDI. Если у вас нет кода или ссылки, вы можете войти с ограниченными возможностями и написать администратору узла запрос на статус пользователя в разделе «Настройки». Администрация узла принимает решение на своё усмотрение. После входа гостем можно сменить узел, если есть другие активные.'),
              key: const ValueKey('guest-intro'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 14, height: 1.35)),
          const SizedBox(height: 12),
          Text(tr('Гостю можно: личная переписка, голосовые, аудио и видео. Нельзя: группы, выход в интернет через узел, файлы больше 30 МБ, больше 20 файлов или 300 МБ в сутки. Данные гостя на узле стираются через сутки без активности.'),
              key: const ValueKey('guest-limits'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 13, height: 1.35)),
          const SizedBox(height: 16),
          if (_error != null)
            Padding(padding: const EdgeInsets.only(bottom: 12), child: Text(_error!, key: const ValueKey('guest-error'), style: const TextStyle(color: Colors.redAccent))),
          if (nodes == null)
            const Center(child: CircularProgressIndicator(color: AppTheme.accent))
          else if (nodes.isEmpty)
            Text(tr('Сейчас нет узлов, принимающих гостей. Попросите код или ссылку у администратора узла.'), key: const ValueKey('guest-empty'), style: const TextStyle(color: Colors.orangeAccent))
          else
            for (final n in nodes)
              Padding(
                padding: const EdgeInsets.only(bottom: 10),
                child: FilledButton(
                  key: ValueKey('guest-node-${n.host}:${n.port}'),
                  onPressed: _busy != null ? null : () => _enter(n),
                  child: Text(_busy == n ? tr('Входим…') : trf('Войти гостем на узел «{name}»', {'name': n.name})),
                ),
              ),
        ],
      ),
    );
  }
}
