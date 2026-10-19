/// Кошелёк РАЗРАБОТКИ проекта YANDI (не узла и не его владельца): серверы, подписки на ИИ-инструменты и т. п. Вшит в приложение и не берётся
/// из сети. Адрес проверен контрольной суммой Base58Check (тест `support_wallet_test.dart`): опечатка в нём — потерянные деньги.
class SupportWallet {
  final String coin, network, address;
  const SupportWallet(this.coin, this.network, this.address);
}

const supportWallets = <SupportWallet>[
  SupportWallet('USDT', 'Tron (TRC20)', 'TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB'),
];
