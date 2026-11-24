import 'dart:convert';
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

/// Keccak-256 (не SHA3-256): нужен для контрольной суммы регистра адреса EVM (EIP-55).
List<int> keccak256(List<int> data) {
  final rc = <String>[
    '0000000000000001', '0000000000008082', '800000000000808A', '8000000080008000', '000000000000808B', '0000000080000001', '8000000080008081', '8000000000008009',
    '000000000000008A', '0000000000000088', '0000000080008009', '000000008000000A', '000000008000808B', '800000000000008B', '8000000000008089', '8000000000008003',
    '8000000000008002', '8000000000000080', '000000000000800A', '800000008000000A', '8000000080008081', '8000000000008080', '0000000080000001', '8000000080008008',
  ].map((h) => BigInt.parse(h, radix: 16)).toList();
  const rot = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]];
  final mask = (BigInt.one << 64) - BigInt.one;
  BigInt rol(BigInt x, int n) => n == 0 ? x : ((x << n) | (x >> (64 - n))) & mask;
  var s = List.generate(5, (_) => List.filled(5, BigInt.zero));
  final p = List<int>.from(data)..add(1);
  while (p.length % 136 != 0) {
    p.add(0);
  }
  p[p.length - 1] |= 0x80;
  for (var i = 0; i < p.length; i += 136) {
    for (var j = 0; j < 17; j++) {
      var lane = BigInt.zero;
      for (var k = 7; k >= 0; k--) {
        lane = (lane << 8) | BigInt.from(p[i + 8 * j + k]);
      }
      s[j % 5][j ~/ 5] ^= lane;
    }
    for (final r in rc) {
      final c = List.generate(5, (x) => s[x][0] ^ s[x][1] ^ s[x][2] ^ s[x][3] ^ s[x][4]);
      final d = List.generate(5, (x) => c[(x + 4) % 5] ^ rol(c[(x + 1) % 5], 1));
      s = List.generate(5, (x) => List.generate(5, (y) => s[x][y] ^ d[x]));
      final b = List.generate(5, (_) => List.filled(5, BigInt.zero));
      for (var x = 0; x < 5; x++) {
        for (var y = 0; y < 5; y++) {
          b[y][(2 * x + 3 * y) % 5] = rol(s[x][y], rot[x][y]);
        }
      }
      s = List.generate(5, (x) => List.generate(5, (y) => b[x][y] ^ ((~b[(x + 1) % 5][y] & mask) & b[(x + 2) % 5][y])));
      s[0][0] ^= r;
    }
  }
  final out = <int>[];
  for (var j = 0; j < 4; j++) {
    var lane = s[j % 5][j ~/ 5];
    for (var k = 0; k < 8; k++) {
      out.add((lane & BigInt.from(255)).toInt());
      lane >>= 8;
    }
  }
  return out;
}

/// Адрес EVM: 0x + 40 hex, регистр букв — контрольная сумма EIP-55; целиком в одном регистре не принимается.
bool validEvmAddress(String a) {
  if (!RegExp(r'^0x[0-9a-fA-F]{40}$').hasMatch(a)) return false;
  final low = a.substring(2).toLowerCase();
  final k = keccak256(utf8.encode(low));
  final sb = StringBuffer();
  for (var i = 0; i < 40; i++) {
    final nib = i.isEven ? k[i ~/ 2] >> 4 : k[i ~/ 2] & 15;
    final ch = low[i];
    sb.write(RegExp(r'[a-f]').hasMatch(ch) && nib >= 8 ? ch.toUpperCase() : ch);
  }
  return sb.toString() == a.substring(2);
}

void main() {
  test('вшитые кошельки: Tron проходит Base58Check, EVM — EIP-55; опечатка и подмена сети ловятся', () {
    expect(supportWallets.map((w) => w.network).toList(), ['Tron (TRC-20)', 'Ethereum (ERC-20)', 'BNB Smart Chain (BEP-20)', 'Polygon (PoS)']);
    for (final w in supportWallets) {
      final ok = w.address.startsWith('0x') ? validEvmAddress(w.address) : validTronAddress(w.address);
      expect(ok, isTrue, reason: 'опечатка в адресе = потерянные деньги: ${w.network}');
    }
    expect(supportWallets.map((w) => w.id).toSet().length, 4);
    final tron = supportWallets.first.address;
    expect(validTronAddress(tron.replaceFirst('L', 'M')), isFalse);
    expect(validEvmAddress(supportWallets[1].address.toLowerCase()), isFalse);
  });

  test('кошельки из подписанного манифеста: верные берутся, мусор пропускается, пусто — остаются вшитые', () {
    final l = SupportWallet.listFromSigned([
      {'asset': 'USDT', 'network': 'Tron (TRC-20)', 'address': supportWallets[0].address},
      {'asset': 'USDT', 'network': 'Polygon (PoS)', 'address': 'не адрес'},
      {'asset': 'USDT/USDC', 'network': 'Polygon (PoS)', 'address': supportWallets[3].address},
      'строка',
    ]);
    expect(l.map((w) => w.network).toList(), ['Tron (TRC-20)', 'Polygon (PoS)']);
    expect(SupportWallet.listFromSigned(null), isEmpty);
    expect(SupportWallet.listFromSigned({'asset': 'USDT'}), isEmpty, reason: 'не список');
  });

  testWidgets('экран поддержки: монета, сеть, адрес, QR, предупреждение о сети; копирование', (t) async {
    final copied = <String>[];
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') copied.add((call.arguments as Map)['text'] as String);
      return null;
    });
    addTearDown(() => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, null));
    t.view.physicalSize = const Size(800, 6000);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    await t.pumpWidget(const MaterialApp(home: SupportScreen()));
    // четыре сети: у каждой своя подпись, QR и кнопки; адрес EVM один на три сети
    for (final n in ['Tron (TRC-20)', 'Ethereum (ERC-20)', 'BNB Smart Chain (BEP-20)', 'Polygon (PoS)']) {
      expect(find.textContaining('сеть $n'), findsOneWidget, reason: n);
    }
    expect(find.text('TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'), findsOneWidget);
    expect(find.text('0x00afCEFc2428B3D00f7d6B739C071339de4c2521'), findsNWidgets(3));
    expect(find.byKey(const ValueKey('support-qr-USDT|Tron (TRC-20)')), findsOneWidget);
    expect(find.byKey(const ValueKey('support-qr-USDT/USDC|Polygon (PoS)')), findsOneWidget);
    expect(find.textContaining('Отправляйте только USDT в сети Tron (TRC-20)'), findsOneWidget);
    expect(find.textContaining('Отправляйте только USDT/USDC в сети BNB Smart Chain (BEP-20)'), findsOneWidget);
    expect(find.textContaining('а не владельца вашего узла'), findsOneWidget, reason: 'не путать с узлом');
    await t.tap(find.byKey(const ValueKey('support-copy-USDT/USDC|Polygon (PoS)')));
    await t.pump();
    expect(copied, ['0x00afCEFc2428B3D00f7d6B739C071339de4c2521']);
    expect(find.text('Адрес скопирован. Вставьте его в свой кошелёк.'), findsOneWidget, reason: 'человек видит, что адрес в буфере');
    await t.pumpWidget(const MaterialApp(home: SupportScreen(key: ValueKey('empty'), wallets: [])));
    expect(find.byKey(const ValueKey('support-empty')), findsOneWidget);
  });

  testWidgets('«Отправить адрес…» передаёт именно адрес системному окну «Поделиться»; при отказе адрес остаётся в буфере', (t) async {
    final shared = <String>[];
    final copied = <String>[];
    var shareWorks = true;
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(const MethodChannel('com.yandi.yandi_mobile/files'), (call) async {
      if (call.method == 'shareText') {
        shared.add((call.arguments as Map)['text'] as String);
        if (!shareWorks) throw PlatformException(code: 'share');
        return true;
      }
      return null;
    });
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') copied.add((call.arguments as Map)['text'] as String);
      return null;
    });
    addTearDown(() {
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(const MethodChannel('com.yandi.yandi_mobile/files'), null);
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(SystemChannels.platform, null);
    });
    t.view.physicalSize = const Size(800, 1600);
    t.view.devicePixelRatio = 1.0;
    addTearDown(t.view.reset);
    await t.pumpWidget(const MaterialApp(home: SupportScreen()));
    await t.tap(find.byKey(const ValueKey('support-share-USDT|Tron (TRC-20)')));
    await t.pumpAndSettle();
    expect(shared, ['TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'], reason: 'пересылается только адрес, без добавок');
    expect(copied, isEmpty);
    shareWorks = false;
    await t.tap(find.byKey(const ValueKey('support-share-USDT|Tron (TRC-20)')));
    await t.pumpAndSettle();
    expect(copied, ['TPeLtLuE9qx64neroHn14LjWwRn5jmupHY'], reason: 'если «Поделиться» недоступно — адрес копируется');
  });
}
