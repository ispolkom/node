import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/home_blocks.dart';

const _alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

/// Base58Check: адрес Tron — 25 байт (версия 0x41 + 20 байт + 4 байта двойной SHA-256).
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
  test('вшитый кошелёк проекта: адрес Tron проходит Base58Check, сеть и монета названы, адрес один', () {
    final wallets = Wallet.parse(File('assets/support.json').readAsStringSync());
    expect(wallets.length, 1);
    final w = wallets.single;
    expect(w.coin, 'USDT');
    expect(w.network, 'Tron (TRC20)');
    expect(w.address, 'TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB');
    expect(validTronAddress(w.address), isTrue, reason: 'опечатка в адресе = потерянные деньги: контрольная сумма обязана сходиться');
  });

  test('проверка Base58Check ловит опечатку и чужую сеть', () {
    const good = 'TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB';
    expect(validTronAddress(good), isTrue);
    expect(validTronAddress('TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsC'), isFalse, reason: 'последний знак изменён');
    expect(validTronAddress('TLMwvkp87wpcE7HkWJ6NeSaK6uuSEq4Es'), isFalse, reason: 'короче');
    expect(validTronAddress('0LMwvkp87wpcE7HkWJ6NeSaK6uuSEq4EsB'), isFalse);
    expect(validTronAddress(good.replaceFirst('W', 'V')), isFalse, reason: 'один знак подменён');
    expect(jsonDecode(File('assets/support.json').readAsStringSync()), isA<Map>());
  });
}
