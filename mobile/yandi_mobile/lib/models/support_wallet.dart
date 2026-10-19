/// Кошелёк РАЗРАБОТКИ проекта YANDI (не узла и не его владельца): серверы, подписки на ИИ-инструменты и т. п. Вшит в приложение и не берётся
/// из сети. Адрес проверен контрольной суммой Base58Check (тест `support_wallet_test.dart`): опечатка в нём — потерянные деньги.
class SupportWallet {
  final String coin, network, address;
  const SupportWallet(this.coin, this.network, this.address);

  /// Ключ строки в списке: адрес EVM один на несколько сетей, поэтому различаем по монете и сети.
  String get id => '$coin|$network';

  /// Один кошелёк из подписанного манифеста (`{asset, network, address}`); неверный вид — null.
  static SupportWallet? fromSigned(Object? m) {
    if (m is! Map) return null;
    final c = '${m['asset'] ?? ''}'.trim(), n = '${m['network'] ?? ''}'.trim(), a = '${m['address'] ?? ''}'.trim();
    if (c.isEmpty || c.length > 24 || n.isEmpty || n.length > 32) return null;
    // Tron (T…, 34 знака) или EVM (0x + 40 hex): контрольные суммы проверены при выпуске, здесь только вид
    if (!RegExp(r'^(T[1-9A-HJ-NP-Za-km-z]{33}|0x[0-9a-fA-F]{40})$').hasMatch(a)) return null;
    return SupportWallet(c, n, a);
  }

  /// Список кошельков из ответа узла: до 8 верных; пусто — остаётся вшитый список.
  static List<SupportWallet> listFromSigned(Object? m) {
    if (m is! List) return const [];
    return [for (final x in m.take(8)) SupportWallet.fromSigned(x)].whereType<SupportWallet>().toList();
  }
}

const supportWallets = <SupportWallet>[
  SupportWallet('USDT', 'Tron (TRC-20)', 'TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'),
  SupportWallet('USDT/USDC', 'Ethereum (ERC-20)', '0x00afCEFc2428B3D00f7d6B739C071339de4c2521'),
  SupportWallet('USDT/USDC', 'BNB Smart Chain (BEP-20)', '0x00afCEFc2428B3D00f7d6B739C071339de4c2521'),
  SupportWallet('USDT/USDC', 'Polygon (PoS)', '0x00afCEFc2428B3D00f7d6B739C071339de4c2521'),
];
