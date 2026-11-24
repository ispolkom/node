import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/media_player.dart';

Future<(int, List<int>, HttpHeaders)> fetch(
  Uri url, {
  String method = 'GET',
  String? range,
}) async {
  final c = HttpClient();
  final req = await c.openUrl(method, url);
  if (range != null) req.headers.set(HttpHeaders.rangeHeader, range);
  final res = await req.close();
  final body = <int>[];
  await for (final part in res) {
    body.addAll(part);
  }
  c.close();
  return (res.statusCode, body, res.headers);
}

void main() {
  test('loopback media serves bytes, ranges and refuses other paths', () async {
    final data = Uint8List.fromList(List.generate(100, (i) => i));
    final m = LoopbackMedia(data, 'video/mp4');
    final url = await m.start();
    expect(url.host, '127.0.0.1');
    final all = await fetch(url);
    expect(all.$1, 200);
    expect(all.$2.length, 100);
    expect(all.$3.value('accept-ranges'), 'bytes');
    final part = await fetch(url, range: 'bytes=10-19');
    expect(part.$1, 206);
    expect(part.$2, List.generate(10, (i) => 10 + i));
    expect(part.$3.value('content-range'), 'bytes 10-19/100');
    final tail = await fetch(url, range: 'bytes=-5');
    expect(tail.$2, [95, 96, 97, 98, 99]);
    final open = await fetch(url, range: 'bytes=90-');
    expect(open.$2.length, 10);
    final bad = await fetch(url, range: 'bytes=500-600');
    expect(bad.$1, 416);
    final head = await fetch(url, method: 'HEAD');
    expect(head.$1, 200);
    expect(head.$2, isEmpty);
    final wrong = await fetch(url.replace(path: '/guess'));
    expect(wrong.$1, 404);
    await m.stop();
    expect(data.every((b) => b == 0), true);
    await expectLater(fetch(url), throwsA(anything));
  });

  test('проигрыватель запускается без прокси из окружения: петля 127.0.0.1 идёт напрямую', () {
    final env = MediaPlayerLauncher.playerEnvironment();
    for (final k in ['http_proxy', 'https_proxy', 'all_proxy', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY']) {
      expect(env[k], '', reason: '$k должен быть сброшен');
    }
    expect(env['no_proxy'], contains('127.0.0.1'));
    expect(env['NO_PROXY'], contains('localhost'));
  });
}
