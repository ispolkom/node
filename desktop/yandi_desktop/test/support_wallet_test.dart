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
  test('Keccak-256 даёт известные значения (пустая строка и «abc»)', () {
    String hex(List<int> b) => b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();
    expect(hex(keccak256(const [])), 'c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470');
    expect(hex(keccak256(utf8.encode('abc'))), '4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45');
  });

  test('вшитые кошельки проекта: Tron проходит Base58Check, адрес EVM — EIP-55; у каждого названы монета и сеть', () {
    final wallets = Wallet.parse(File('assets/support.json').readAsStringSync());
    expect(wallets.map((w) => w.network).toList(), ['Tron (TRC-20)', 'Ethereum (ERC-20)', 'BNB Smart Chain (BEP-20)', 'Polygon (PoS)']);
    for (final w in wallets) {
      expect(w.coin.isNotEmpty, isTrue);
      final ok = w.address.startsWith('0x') ? validEvmAddress(w.address) : validTronAddress(w.address);
      expect(ok, isTrue, reason: 'опечатка в адресе = потерянные деньги: ${w.network}');
    }
    expect(wallets.where((w) => w.address.startsWith('0x')).map((w) => w.address).toSet().length, 1, reason: 'один адрес EVM на три сети');
    expect(wallets.map((w) => w.id).toSet().length, 4, reason: 'строки различимы по монете и сети');
  });

  test('проверки ловят опечатку, испорченный регистр и чужую сеть', () {
    const tron = 'TPeLtLuE9qx64neroHn14LjWwRn5jmupHY';
    const evm = '0x00afCEFc2428B3D00f7d6B739C071339de4c2521';
    expect(validTronAddress(tron), isTrue);
    expect(validTronAddress('${tron.substring(0, 33)}Z'), isFalse);
    expect(validTronAddress(tron.substring(0, 33)), isFalse, reason: 'короче');
    expect(validEvmAddress(evm), isTrue);
    expect(validEvmAddress(evm.toLowerCase()), isFalse, reason: 'без контрольного регистра не принимается');
    expect(validEvmAddress(evm.replaceFirst('afCE', 'AFce')), isFalse, reason: 'регистр испорчен');
    expect(validEvmAddress(tron), isFalse);
    expect(jsonDecode(File('assets/support.json').readAsStringSync()), isA<Map>());
  });
}
