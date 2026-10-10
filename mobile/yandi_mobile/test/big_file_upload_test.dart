import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_crypto.dart';
import 'package:yandi_mobile/models/trusted_node.dart';
import 'package:yandi_mobile/services/api_service.dart';
import 'package:yandi_mobile/services/file_transfer_service.dart';

/// Подставной узел: куски падают в память, ответ приходит со случайной задержкой (как у настоящей сети).
class FakeApi extends ApiService {
  FakeApi() : super(TrustedNode(id: 'n', name: 'n', host: '127.0.0.1', port: 1, fingerprint: '00' * 32, token: 't', addedAt: 0));
  final chunks = <int, List<int>>{};
  int inFlight = 0, peak = 0, started = 0, completed = 0;
  int? failOnceAt;
  bool _failed = false;
  final _rnd = Random(7);

  @override
  Future<String?> startFileTransfer({required String toPeerId, required String fileName, required int fileSize, required int totalChunks}) async =>
      '0123456789abcdef0123456789abcdef';

  @override
  Future<bool> uploadChunk(String transferId, int idx, List<int> bytes) async {
    started++;
    inFlight++;
    if (inFlight > peak) peak = inFlight;
    await Future<void>.delayed(Duration(milliseconds: 5 + _rnd.nextInt(25)));
    inFlight--;
    if (failOnceAt == idx && !_failed) { _failed = true; return false; } // одна временная неудача: повтор должен спасти
    chunks[idx] = bytes;
    completed++;
    return true;
  }

  @override
  Future<bool> completeTransfer(String transferId) async => true;
}

void main() {
  late Directory dir;
  setUp(() => dir = Directory.systemTemp.createTempSync('yandi_up'));
  tearDown(() => dir.deleteSync(recursive: true));

  Future<File> make(int size) async {
    final f = File('${dir.path}/f.bin');
    final r = Random(3);
    await f.writeAsBytes(Uint8List.fromList(List<int>.generate(size, (_) => r.nextInt(256))));
    return f;
  }

  test('большой файл: куски по 1 МБ, заливка параллельная, на выходе тот же файл и та же сумма', () async {
    final f = await make(12 * 1024 * 1024 + 12345);
    final api = FakeApi();
    final svc = FileTransferService(api);
    final offer = await svc.prepare(toPeerId: 'p', path: f.path);
    expect(offer.chunks, 13);
    final progress = <double>[];
    final done = await svc.upload(offer, f.path, onProgress: progress.add);
    expect(api.chunks.length, 13);
    expect(api.peak, greaterThan(1), reason: 'куски должны лететь одновременно');
    expect(api.peak, lessThanOrEqualTo(bigParallel));
    expect(progress.last, closeTo(1.0, 1e-9));
    // собираем обратно по порядку, расшифровываем ключом из предложения и сверяем
    final out = BytesBuilder();
    for (var i = 0; i < offer.chunks; i++) {
      out.add(await FileCrypto.decryptChunk(_b64(offer.key), offer.tid, i, offer.chunks, api.chunks[i]!));
    }
    final got = out.toBytes();
    expect(got.length, 12 * 1024 * 1024 + 12345);
    expect(crypto.sha256.convert(got).toString(), done.sha);
    expect(crypto.sha256.convert(await f.readAsBytes()).toString(), done.sha);
  });

  test('мелкий файл: куски по 256 КБ, строго по одному', () async {
    final f = await make(1024 * 1024 + 1);
    final api = FakeApi();
    final svc = FileTransferService(api);
    final offer = await svc.prepare(toPeerId: 'p', path: f.path);
    expect(offer.chunks, 5);
    await svc.upload(offer, f.path);
    expect(api.peak, 1);
  });

  test('временная неудача одного куска не ломает загрузку (повтор)', () async {
    final f = await make(8 * 1024 * 1024);
    final api = FakeApi()..failOnceAt = 3;
    final svc = FileTransferService(api);
    final offer = await svc.prepare(toPeerId: 'p', path: f.path);
    final done = await svc.upload(offer, f.path);
    expect(api.chunks.length, 8);
    expect(done.sha.length, 64);
  });
}

List<int> _b64(String s) => base64Decode(s);
