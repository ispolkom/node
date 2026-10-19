import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'package:crypto/crypto.dart' as crypto;
import 'package:cryptography/cryptography.dart';
import '../l10n_core.dart';
import '../models/guest_node.dart';
import 'guest_pair.dart';

/// Ответ узла на `GET /mobile/hello` и отпечаток сертификата, по которому он получен.
class HelloReply {
  final Map<String, dynamic> body;
  final String certFingerprint;
  const HelloReply(this.body, this.certFingerprint);
}

abstract class HelloTransport {
  Future<HelloReply> hello(String host, int port, String nonceHex);
}

/// Соединение по TLS без проверки сертификата ТОЛЬКО для этого запроса: отпечаток полученного сертификата запоминается и сверяется с тем, что
/// узел подписал своим ключом из подписанного списка. Подставной узел так не представится (нет ключа), а чужой сертификат не совпадёт с подписанным.
class IoHelloTransport implements HelloTransport {
  @override
  Future<HelloReply> hello(String host, int port, String nonceHex) async {
    String? fp;
    final c = HttpClient(context: SecurityContext(withTrustedRoots: false))
      ..connectionTimeout = const Duration(seconds: 8)
      ..badCertificateCallback = (X509Certificate cert, String h, int p) {
        fp = crypto.sha256.convert(cert.der).toString();
        return true;
      };
    try {
      final req = await c.getUrl(Uri(scheme: 'https', host: host, port: port, path: '/mobile/hello', queryParameters: {'nonce': nonceHex})).timeout(const Duration(seconds: 10));
      final res = await req.close().timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) throw const HttpException('hello');
      final bytes = <int>[];
      await for (final chunk in res.timeout(const Duration(seconds: 10))) {
        bytes.addAll(chunk);
        if (bytes.length > 4096) throw const HttpException('hello too large');
      }
      final body = jsonDecode(utf8.decode(bytes));
      if (body is! Map<String, dynamic> || fp == null) throw const HttpException('hello');
      return HelloReply(body, fp!);
    } finally {
      c.close(force: true);
    }
  }
}

/// Что подписывает узел, представляясь (как `hello_message` на узле): метка, случайное число клиента, отпечаток сертификата в нижнем регистре.
List<int> helloMessage(List<int> nonce, String tlsFingerprintHex) => [...utf8.encode('YANDI-MOBILE-HELLO-V1\u0000'), ...nonce, ...utf8.encode(tlsFingerprintHex.toLowerCase())];

/// Убедиться, что по адресу из списка отвечает именно узел с номером и ключом из списка: возвращает ПРОВЕРЕННЫЙ отпечаток его сертификата,
/// с которым дальше соединяемся. Любое несоответствие — [GuestPairError].
Future<String> verifyNodeHello(GuestNode n, {HelloTransport? transport, List<int>? nonce}) async {
  final bad = GuestPairError(tr('Узел не подтвердил, что он тот самый из списка: вход отменён.'));
  try {
    final nn = nonce ?? List<int>.generate(16, (_) => Random.secure().nextInt(256));
    final nonceHex = nn.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
    final r = await (transport ?? IoHelloTransport()).hello(n.host, n.port, nonceHex);
    final b = r.body;
    if ('${b['node_id']}'.toLowerCase() != n.id.toLowerCase() || '${b['key']}'.toLowerCase() != n.key.toLowerCase()) throw bad;
    final claimed = '${b['tls_fingerprint']}'.toLowerCase();
    if (claimed != r.certFingerprint.toLowerCase()) throw bad; // подписан не тот сертификат, по которому мы подключились
    final sigHex = '${b['signature']}';
    if (!RegExp(r'^[0-9a-fA-F]{128}$').hasMatch(sigHex) || !RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(n.key)) throw bad;
    List<int> unhex(String h) => [for (var i = 0; i < h.length; i += 2) int.parse(h.substring(i, i + 2), radix: 16)];
    final ok = await Ed25519().verify(helloMessage(nn, claimed), signature: Signature(unhex(sigHex), publicKey: SimplePublicKey(unhex(n.key), type: KeyPairType.ed25519)));
    if (!ok) throw bad;
    return claimed;
  } on GuestPairError {
    rethrow;
  } catch (_) {
    throw GuestPairError(tr('Не удалось связаться с узлом. Проверьте интернет или выберите другой узел.'));
  }
}
