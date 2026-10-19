/// Кошелёк РАЗРАБОТКИ проекта YANDI (не узла и не его владельца): серверы, подписки на ИИ-инструменты и т. п. Вшит в приложение и не берётся
/// из сети. Адрес проверен контрольной суммой Base58Check (тест `support_wallet_test.dart`): опечатка в нём — потерянные деньги.
class SupportWallet {
  final String coin, network, address;
  const SupportWallet(this.coin, this.network, this.address);

  /// Из подписанного манифеста (`{asset, network, address}`); неверный вид — null, остаётся вшитый.
  static SupportWallet? fromSigned(Object? m) {
    if (m is! Map) return null;
    final c = '${m['asset'] ?? ''}'.trim(), n = '${m['network'] ?? ''}'.trim(), a = '${m['address'] ?? ''}'.trim();
    if (c.isEmpty || c.length > 16 || n.length > 32 || !RegExp(r'^[A-Za-z0-9]{26,64}$').hasMatch(a)) return null;
    return SupportWallet(c, n, a);
  }
}

const supportWallets = <SupportWallet>[
  SupportWallet('USDT', 'Tron (TRC20)', 'TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB'),
];
