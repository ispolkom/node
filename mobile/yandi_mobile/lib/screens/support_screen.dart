import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import '../models/support_wallet.dart';
import '../theme.dart';

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
        title: const Text('Поддержать проект', style: TextStyle(color: AppTheme.text)),
        iconTheme: const IconThemeData(color: AppTheme.text),
      ),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Text(
            'YANDI делают энтузиасты, без рекламы и без слежки. Деньги идут на серверы, подписки на инструменты разработки и поддержку проекта: '
            'это кошелёк проекта, а не владельца вашего узла. Пожертвование добровольное, ни к чему не обязывает и не даёт особых прав.',
            style: TextStyle(color: AppTheme.textSecondary, fontSize: 13),
          ),
          const SizedBox(height: 16),
          if (widget.wallets.isEmpty) const Text('Адрес для поддержки пока не указан.', key: ValueKey('support-empty'), style: TextStyle(color: AppTheme.textSecondary)),
          for (final w in widget.wallets) ...[
            Text('${w.coin} · сеть ${w.network}', style: const TextStyle(color: AppTheme.text, fontWeight: FontWeight.bold, fontSize: 16)),
            const SizedBox(height: 10),
            Center(
              child: Container(
                color: Colors.white,
                padding: const EdgeInsets.all(12),
                child: QrImageView(key: ValueKey('support-qr-${w.coin}'), data: w.address, size: 220, backgroundColor: Colors.white),
              ),
            ),
            const SizedBox(height: 10),
            SelectableText(w.address, key: ValueKey('support-address-${w.coin}'), style: const TextStyle(color: AppTheme.text, fontSize: 15)),
            const SizedBox(height: 6),
            Text('Отправляйте только ${w.coin} в сети ${w.network}: перевод в другой сети будет потерян.', style: const TextStyle(color: Colors.orangeAccent, fontSize: 12)),
            TextButton.icon(
              key: ValueKey('support-copy-${w.coin}'),
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: w.address));
                if (mounted) setState(() => copied = w.coin);
              },
              icon: const Icon(Icons.copy, size: 18),
              label: Text(copied == w.coin ? 'Скопировано' : 'Скопировать адрес'),
            ),
            const SizedBox(height: 16),
          ],
        ],
      ),
    );
  }
}
