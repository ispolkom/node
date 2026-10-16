import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:yandi_mobile/services/limited_client.dart';

class _Fake extends http.BaseClient {
  _Fake(this.chunks, {this.declared});
  final List<List<int>> chunks;
  final int? declared;
  @override
  Future<http.StreamedResponse> send(http.BaseRequest request) async =>
      http.StreamedResponse(Stream.fromIterable(chunks), 200, contentLength: declared, request: request);
}

void main() {
  test('обычный ответ читается целиком', () async {
    final c = LimitedClient(_Fake([[1, 2], [3]]), maxBytes: 10);
    final r = await c.get(Uri.parse('https://x/y'));
    expect(r.bodyBytes, [1, 2, 3]);
  });

  test('ответ сверх предела обрывается по ходу чтения', () async {
    final c = LimitedClient(_Fake([List.filled(6, 1), List.filled(6, 1)]), maxBytes: 10);
    await expectLater(c.get(Uri.parse('https://x/y')), throwsA(isA<ResponseTooLarge>()));
  });

  test('заявленная длина сверх предела отвергается до чтения', () async {
    final c = LimitedClient(_Fake([[1]], declared: 1 << 40), maxBytes: 10);
    await expectLater(c.get(Uri.parse('https://x/y')), throwsA(isA<ResponseTooLarge>()));
  });

  test('узел отдал заголовки и замолчал: чтение тела обрывается по сроку, а не висит', () async {
    final stalled = StreamController<List<int>>();
    addTearDown(stalled.close);
    stalled.add([1, 2, 3]);
    final result = readWithDeadline(() async => http.StreamedResponse(stalled.stream, 200), const Duration(milliseconds: 200));
    await expectLater(result, throwsA(isA<TimeoutException>()));
  });
}
