import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/transport/api.dart';

const peer = '0123456789abcdef';
void main() {
  test(
    'Unix incoming null and restricted phone query, node WS send receive cancel',
    () async {
      final dir = await Directory.systemTemp.createTemp('yandi-call-unix-');
      final path = '${dir.path}/ui.sock';
      final server = await HttpServer.bind(
        InternetAddress(path, type: InternetAddressType.unix),
        0,
      );
      final api = UnixApi(path);
      WebSocket? daemon;
      server.listen((r) async {
        if (WebSocketTransformer.isUpgradeRequest(r)) {
          expect(r.uri.path, '/api/media/ws/$peer');
          expect(r.uri.query, isEmpty);
          daemon = await WebSocketTransformer.upgrade(r);
          daemon!.listen((s) {
            daemon!.add(s);
          });
        } else {
          r.response.headers.contentType = ContentType.json;
          r.response.write(
            r.uri.path.endsWith('incoming-call')
                ? 'null'
                : '{"events":[],"last":0}',
          );
          await r.response.close();
        }
      });
      try {
        expect(await api.request('/api/media/incoming-call'), {
          'incoming': null,
        });
        expect(
          (await api.request('/api/mobile/call/events?after=3'))['last'],
          0,
        );
        await expectLater(
          api.request('/api/mobile/call/events?after=3&token=secret'),
          throwsA(isA<ApiFailure>()),
        );
        final ws = await api.openMediaSocket(peer);
        final response = ws.first;
        ws.add('{"type":"hangup","call_id":"call0001"}');
        expect(await response, contains('call0001'));
        api.cancel();
        await ws.done;
      } finally {
        api.cancel();
        await daemon?.close();
        await server.close(force: true);
        await dir.delete(recursive: true);
      }
    },
  );
}
