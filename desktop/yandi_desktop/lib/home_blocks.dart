import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'theme.dart';

/// Закреплённый блок вверху главной: новости и обновления. Данные — из подписанной ленты выпусков, которую проверил узел (`/api/update/status`);
/// текст ограничен по длине и очищен от управляющих знаков, ссылки не открываются.
class NewsBlock extends StatelessWidget {
  final Map<String, dynamic>? update;
  const NewsBlock({super.key, required this.update});

  /// Очищенный текст: без управляющих знаков, не длиннее [max].
  static String clean(Object? v, {int max = 600}) {
    final s = '${v ?? ''}'.replaceAll(RegExp(r'[\u0000-\u0008\u000B-\u001F\u007F]'), '').trim();
    return s.length <= max ? s : '${s.substring(0, max)}…';
  }

  @override
  Widget build(BuildContext context) {
    final u = update;
    final available = u?['available'];
    final hasUpdate = available is Map && available['version'] is String;
    final current = clean(u?['current'], max: 40);
    final notes = hasUpdate ? clean(available['notes']) : '';
    return Container(
      key: const ValueKey('news-block'),
      decoration: BoxDecoration(
        color: Icq.light,
        border: Border(left: BorderSide(color: hasUpdate ? Icq.peer : Icq.header, width: 5), top: Icq.frame.top, right: Icq.frame.right, bottom: Icq.frame.bottom),
      ),
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            const Icon(Icons.push_pin, size: 18, color: Icq.header),
            const SizedBox(width: 8),
            const Expanded(child: Text('Новости и обновления', style: TextStyle(fontSize: 17, fontWeight: FontWeight.bold, color: Icq.text))),
          ]),
          const SizedBox(height: 8),
          if (hasUpdate) ...[
            Text('Доступно обновление YANDI ${clean(available['version'], max: 40)}', key: const ValueKey('news-update'), style: const TextStyle(fontSize: 15, fontWeight: FontWeight.bold, color: Icq.peer)),
            if (notes.isNotEmpty) Padding(padding: const EdgeInsets.only(top: 6), child: Text(notes, key: const ValueKey('news-notes'), style: const TextStyle(color: Icq.text))),
            const SizedBox(height: 6),
            Text(current.isEmpty ? 'Установите выпуск пакетом .deb; узел сам ничего не ставит.' : 'Установлена версия $current. Установите выпуск пакетом .deb; узел сам ничего не ставит.', style: const TextStyle(fontSize: 12, color: Icq.dark)),
          ] else ...[
            Text(current.isEmpty ? 'Здесь будут появляться новости проекта и сообщения об обновлениях.' : 'Установлена версия $current. Новых обновлений нет.', key: const ValueKey('news-none'), style: const TextStyle(color: Icq.text)),
            const SizedBox(height: 4),
            const Text('Здесь появятся новости проекта и сообщения об обновлениях.', style: TextStyle(fontSize: 12, color: Icq.dark)),
          ],
        ],
      ),
    );
  }
}

/// Один кошелёк для поддержки проекта.
class Wallet {
  final String coin, network, address;
  const Wallet(this.coin, this.network, this.address);

  /// Разбор вшитого файла: только непустые адреса из безопасного набора знаков и разумной длины.
  static List<Wallet> parse(String json) {
    try {
      final data = jsonDecode(json);
      if (data is! Map || data['wallets'] is! List) return const [];
      final out = <Wallet>[];
      for (final w in data['wallets'] as List) {
        if (w is! Map) continue;
        final coin = NewsBlock.clean(w['coin'], max: 24), net = NewsBlock.clean(w['network'], max: 16), addr = '${w['address'] ?? ''}'.trim();
        if (coin.isEmpty || addr.length < 20 || addr.length > 128 || !RegExp(r'^[A-Za-z0-9:_\-\.]+$').hasMatch(addr)) continue;
        out.add(Wallet(coin, net, addr));
        if (out.length >= 8) break;
      }
      return out;
    } catch (_) {
      return const [];
    }
  }
}

/// «Поддержать проект»: добровольное пожертвование криптовалютой. Адреса вшиты в сборку (assets/support.json), из сети не берутся.
class SupportBlock extends StatefulWidget {
  /// Загрузчик файла адресов (в тестах подменяется).
  final Future<String> Function() load;
  const SupportBlock({super.key, this.load = _defaultLoad});
  static Future<String> _defaultLoad() => rootBundle.loadString('assets/support.json');
  @override
  State<SupportBlock> createState() => _SupportBlockState();
}

class _SupportBlockState extends State<SupportBlock> {
  List<Wallet> wallets = const [];
  bool loaded = false;
  String? copied;

  @override
  void initState() {
    super.initState();
    widget.load().then((j) {
      if (mounted) setState(() {
        wallets = Wallet.parse(j);
        loaded = true;
      });
    }).catchError((_) {
      if (mounted) setState(() => loaded = true);
    });
  }

  @override
  Widget build(BuildContext context) {
    return Panel(
      key: const ValueKey('support-block'),
      title: 'Поддержать проект',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'YANDI делают энтузиасты, без рекламы и без слежки. Если проект вам полезен, можно поддержать его криптовалютой. Это добровольное '
            'пожертвование: оно ни к чему не обязывает и не даёт особых прав. Адрес вшит в программу; сверьте его с сайтом yandi.su.',
            style: TextStyle(color: Icq.text),
          ),
          const SizedBox(height: 10),
          if (loaded && wallets.isEmpty)
            const Text('Адрес для поддержки пока не указан.', key: ValueKey('support-empty'), style: TextStyle(color: Icq.dark)),
          for (final w in wallets)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(width: 130, child: Text('${w.coin}${w.network.isEmpty ? '' : ' (${w.network})'}', style: const TextStyle(fontWeight: FontWeight.bold, color: Icq.text))),
                  Expanded(child: SelectableText(w.address, key: ValueKey('wallet-${w.coin}'), style: const TextStyle(fontFamily: 'monospace', color: Icq.text))),
                  TextButton(
                    key: ValueKey('copy-${w.coin}'),
                    onPressed: () async {
                      await Clipboard.setData(ClipboardData(text: w.address));
                      if (mounted) setState(() => copied = w.coin);
                    },
                    child: Text(copied == w.coin ? 'Скопировано' : 'Скопировать'),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}
