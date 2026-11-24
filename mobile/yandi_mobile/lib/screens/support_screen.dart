import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import '../models/support_wallet.dart';
import '../services/share_text.dart';
import '../theme.dart';
import '../l10n.dart';

/// «Поддержать проект»: добровольное пожертвование на разработку. Кошелёк вшит в приложение.
class SupportScreen extends StatefulWidget {
  final List<SupportWallet> wallets;
  const SupportScreen({super.key, this.wallets = supportWallets});
  @override
  State<SupportScreen> createState() => _SupportScreenState();
}

class _SupportScreenState extends State<SupportScreen> {
  String? copied;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.bg,
      appBar: AppBar(
        backgroundColor: AppTheme.surface,
        title: Text(tr('Поддержать проект'), style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Text(
            tr('YANDI делают энтузиасты, без рекламы и без слежки. Деньги идут на серверы, подписки на инструменты разработки и поддержку проекта: '
            'это кошелёк проекта, а не владельца вашего узла. Пожертвование добровольное, ни к чему не обязывает и не даёт особых прав.'),
            style: TextStyle(color: AppTheme.textSecondary, fontSize: 13),
          ),
          const SizedBox(height: 16),
          if (widget.wallets.isEmpty) Text(tr('Адрес для поддержки пока не указан.'), key: ValueKey('support-empty'), style: TextStyle(color: AppTheme.textSecondary)),
          for (final w in widget.wallets) ...[
            Text(trf('{c} · сеть {n}', {'c': w.coin, 'n': w.network}), style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.bold, fontSize: 16)),
            const SizedBox(height: 10),
            Center(
              child: Container(
                color: Colors.white,
                padding: const EdgeInsets.all(12),
                child: QrImageView(key: ValueKey('support-qr-${w.id}'), data: w.address, size: 220, backgroundColor: Colors.white),
              ),
            ),
            const SizedBox(height: 10),
            SelectableText(w.address, key: ValueKey('support-address-${w.id}'), style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            const SizedBox(height: 6),
            Text(trf('Отправляйте только {c} в сети {n}: перевод в другой сети будет потерян.', {'c': w.coin, 'n': w.network}), style: const TextStyle(color: Colors.orangeAccent, fontSize: 12)),
            TextButton.icon(
              key: ValueKey('support-copy-${w.id}'),
              onPressed: () async {
                // адрес идёт в системный буфер обмена: его можно вставить в любое другое приложение (кошелёк, мессенджер)
                await Clipboard.setData(ClipboardData(text: w.address));
                if (!mounted) return;
                setState(() => copied = w.id);
                ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Адрес скопирован. Вставьте его в свой кошелёк.'))));
              },
              icon: const Icon(Icons.copy, size: 18),
              label: Text(copied == w.id ? tr('Скопировано') : tr('Скопировать адрес')),
            ),
            TextButton.icon(
              key: ValueKey('support-share-${w.id}'),
              onPressed: () async {
                final ok = await shareText(w.address);
                if (!ok && mounted) {
                  // системное окно «Поделиться» недоступно — адрес хотя бы в буфере
                  await Clipboard.setData(ClipboardData(text: w.address));
                  if (mounted) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(tr('Переслать не удалось: адрес скопирован в буфер обмена.'))));
                }
              },
              icon: const Icon(Icons.share, size: 18),
              label: Text(tr('Отправить адрес…')),
            ),
            const SizedBox(height: 16),
          ],
        ],
      ),
    );
  }
}
