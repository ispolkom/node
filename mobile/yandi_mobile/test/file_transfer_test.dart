import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_crypto.dart';
import 'package:yandi_mobile/models/file_offer.dart';
import 'package:yandi_mobile/services/file_transfer_service.dart';

void main() {
  const tid = '0123456789abcdef0123456789abcdef';
  final key = FileCrypto.newKey();
  final plain = Uint8List.fromList(List<int>.generate(5000, (i) => i * 7 % 256));

  test('a chunk survives the round trip and is 28 bytes longer on the wire', () async {
    final wire = await FileCrypto.encryptChunk(key, tid, 3, 10, plain);
    expect(wire.length, plain.length + 28);
    expect(await FileCrypto.decryptChunk(key, tid, 3, 10, wire), plain);
  });

  test('the same chunk encrypted twice differs (fresh nonce)', () async {
    final a = await FileCrypto.encryptChunk(key, tid, 0, 1, plain);
    final b = await FileCrypto.encryptChunk(key, tid, 0, 1, plain);
    expect(a, isNot(b));
  });

  test('a changed byte is refused', () async {
    final wire = await FileCrypto.encryptChunk(key, tid, 0, 2, plain);
    wire[100] ^= 1;
    expect(() => FileCrypto.decryptChunk(key, tid, 0, 2, wire), throwsA(anything));
  });

  test('a chunk moved to another place, another count or another transfer is refused', () async {
    final wire = await FileCrypto.encryptChunk(key, tid, 1, 4, plain);
    expect(() => FileCrypto.decryptChunk(key, tid, 2, 4, wire), throwsA(anything));
    expect(() => FileCrypto.decryptChunk(key, tid, 1, 3, wire), throwsA(anything));
    expect(() => FileCrypto.decryptChunk(key, 'ffffffffffffffffffffffffffffffff', 1, 4, wire), throwsA(anything));
  });

  test('another key is refused', () async {
    final wire = await FileCrypto.encryptChunk(key, tid, 0, 1, plain);
    expect(() => FileCrypto.decryptChunk(FileCrypto.newKey(), tid, 0, 1, wire), throwsA(anything));
  });

  test('the offer round trips and carries no path', () {
    final o = FileOffer(tid: tid, name: 'photo.jpg', size: 123456, chunks: 1, mime: 'image/jpeg', key: base64.encode(key), sha: 'ab' * 32);
    final back = FileOffer.tryParse(o.encode())!;
    expect(back.tid, tid);
    expect(back.name, 'photo.jpg');
    expect(back.size, 123456);
    expect(back.sha, 'ab' * 32);
    expect(back.isImage, true);
    expect(FileOffer.preview(o.encode()), '📎 photo.jpg');
  });

  test('ordinary text and broken offers are not offers', () {
    expect(FileOffer.tryParse('привет'), isNull);
    expect(FileOffer.tryParse('${FileOffer.marker}{не json'), isNull);
    expect(FileOffer.tryParse('${FileOffer.marker}{"tid":"short","key":"AAAA"}'), isNull);
    expect(FileOffer.preview('привет'), 'привет');
  });

  test('names are made safe', () {
    expect(FileTransferService.safeName('../../etc/passwd'), 'passwd');
    expect(FileTransferService.safeName(r'C:\dir\a.txt'), 'a.txt');
    expect(FileTransferService.safeName('.hidden'), 'hidden');
    expect(FileTransferService.safeName(''), 'file');
    expect(FileTransferService.safeName('a\u0000b<c>.txt').contains(RegExp(r'[\u0000<>]')), false);
  });
}
