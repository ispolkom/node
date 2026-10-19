import 'package:crypto/crypto.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/support_wallet.dart';
import 'package:yandi_mobile/screens/support_screen.dart';

const _alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

/// Base58Check: адрес Tron — 25 байт (версия 0x41 + 20 байт + 4 байта двойного SHA-256).
bool validTronAddress(String a) {
  if (a.length != 34 || !a.startsWith('T')) return false;
  var n = BigInt.zero;
  for (final c in a.split('')) {
    final i = _alphabet.indexOf(c);
    if (i < 0) return false;
    n = n * BigInt.from(58) + BigInt.from(i);
  }
  final bytes = List<int>.filled(25, 0);
  for (var i = 24; i >= 0; i--) {
    bytes[i] = (n & BigInt.from(255)).toInt();
    n >>= 8;
  }
  if (n != BigInt.zero || bytes[0] != 0x41) return false;
  final sum = sha256.convert(sha256.convert(bytes.sublist(0, 21)).bytes).bytes.sublist(0, 4);
  for (var i = 0; i < 4; i++) {
    if (sum[i] != bytes[21 + i]) return false;
  }
  return true;
}

void main() {
  test('вшитый кошелёк: адрес Tron проходит Base58Check; опечатка ловится', () {
    expect(supportWallets.length, 1);
    final w = supportWallets.single;
    expect((w.coin, w.network, w.address), ('USDT', 'Tron (TRC20)', 'TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB'));
    expect(validTronAddress(w.address), isTrue, reason: 'опечатка в адресе = потерянные деньги');
    expect(validTronAddress(w.address.replaceFirst('W', 'V')), isFalse);
    expect(validTronAddress('${w.address.substring(0, 33)}C'), isFalse);
  });

  testWidgets('экран поддержки: монета, сеть, адрес, QR, предупреждение о сети; копирование', (t) async {
    final copied = <String>[];
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') copied.add((call.arguments as Map)['text'] as String);
      return null;
    });
    addTearDown(() => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, null));
    await t.pumpWidget(const MaterialApp(home: SupportScreen()));
    expect(find.textContaining('USDT · сеть Tron (TRC20)'), findsOneWidget);
    expect(find.text('TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB'), findsOneWidget);
    expect(find.byKey(const ValueKey('support-qr-USDT')), findsOneWidget);
    expect(find.textContaining('Отправляйте только USDT в сети Tron (TRC20)'), findsOneWidget);
    expect(find.textContaining('а не владельца вашего узла'), findsOneWidget, reason: 'не путать с узлом');
    await t.scrollUntilVisible(find.byKey(const ValueKey('support-copy-USDT')), 200, scrollable: find.byType(Scrollable).first);
    await t.tap(find.byKey(const ValueKey('support-copy-USDT')));
    await t.pump();
    expect(copied, ['TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB']);
    await t.pumpWidget(const MaterialApp(home: SupportScreen(key: ValueKey('empty'), wallets: [])));
    expect(find.byKey(const ValueKey('support-empty')), findsOneWidget);
  });
}
