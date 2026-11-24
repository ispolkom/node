import 'dart:async';
import 'package:http/http.dart' as http;

/// Предел размера ответа узла: сопряжённый, но неисправный или взломанный узел не должен заставить телефон копить гигабайты
/// в памяти до проверки подписей. Самые большие законные ответы — куски файлов (~1 МиБ) и картинки профилей.
const int kMaxResponseBytes = 8 * 1024 * 1024;

class ResponseTooLarge implements Exception {
  final int limit;
  const ResponseTooLarge(this.limit);
  @override
  String toString() => 'Ответ узла больше допустимых $limit байт';
}

/// Клиент, который обрывает ответ, не дожидаясь конца, как только он перерос предел (по заголовку длины и по ходу чтения).
class LimitedClient extends http.BaseClient {
  LimitedClient(this._inner, {this.maxBytes = kMaxResponseBytes});
  final http.Client _inner;
  final int maxBytes;

  @override
  Future<http.StreamedResponse> send(http.BaseRequest request) async {
    final res = await _inner.send(request);
    final declared = res.contentLength;
    if (declared != null && declared > maxBytes) {
      unawaited(res.stream.listen((_) {}).cancel());
      throw ResponseTooLarge(maxBytes);
    }
    var seen = 0;
    final limited = res.stream.transform(StreamTransformer<List<int>, List<int>>.fromHandlers(
      handleData: (chunk, sink) {
        seen += chunk.length;
        if (seen > maxBytes) {
          sink.addError(ResponseTooLarge(maxBytes));
          sink.close();
        } else {
          sink.add(chunk);
        }
      },
    ));
    return http.StreamedResponse(limited, res.statusCode,
        contentLength: res.contentLength,
        request: res.request,
        headers: res.headers,
        isRedirect: res.isRedirect,
        persistentConnection: res.persistentConnection,
        reasonPhrase: res.reasonPhrase);
  }

  @override
  void close() => _inner.close();
}

/// Весь ответ — заголовки И тело — под одним сроком. Таймаут только на `send` не защищает: узел может отдать заголовки и замолчать.
Future<http.Response> readWithDeadline(Future<http.StreamedResponse> Function() send, Duration limit) =>
    (() async => http.Response.fromStream(await send()))().timeout(limit);
