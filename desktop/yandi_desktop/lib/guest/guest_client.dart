import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:cryptography/cryptography.dart';
import '../tombstone.dart';
import 'guest_bootstrap.dart';

// Самостоятельный гостевой клиент оболочки ПК: тот же обмен с узлом, что у телефона (mobile/yandi_mobile), без узла на этом ПК.
// Формат на проводе совпадает с телефоном побайтно (E4-конверт, подпись связки ключей, кадры сокета); всё хранится только в памяти:
// гость живёт на узле сутки, перезапуск оболочки — новый гость. Ключи узла и собеседников закрепляются в памяти на время сеанса.

/// Ошибка входа или работы с узлом с готовым текстом для человека (русский, ключ перевода в translations.dart).
class GuestError implements Exception {
  final String message;
  const GuestError(this.message);
  @override
  String toString() => message;
}

// ── Кадры сокета (как ws_service.dart) ──────────────────────────────────────
const int ftPing = 0x01;
const int ftPong = 0x02;
const int ftChatMsg = 0x10;
const int ftPeerStatus = 0x12;
const int ftPeerOffline = 0x14;
/// Намёк «устройства убраны с узла»: `[0x17][8 номер LE]`; сам список — `GET /mobile/forgotten` (как на телефоне).
const int ftForgotten = 0x17;
const int ftSendMsg = 0x30;

/// Сколько сообщений гость отправляет за скользящие сутки (как `AppState.guestMessagesPerDay` на телефоне).
const int guestMessagesPerDay = 10;

/// Версия протокола гостя на ПК совпадает с версией телефона (`kAppVersion`) и поднимается вместе с ней: по ней узел отзывает сборки (`min_app`).
const String kGuestAppVersion = '1.0.0-b6';

/// Старше ли версия [a], чем [b] (числа по порядку: `1.0.0-b4` → 1,0,0,4); как `versionOlder` на телефоне.
bool versionOlder(String a, String b) {
  List<int> parts(String v) => [for (final m in RegExp(r'\d+').allMatches(v)) int.parse(m.group(0)!)];
  final x = parts(a), y = parts(b);
  for (var i = 0; i < x.length || i < y.length; i++) {
    final p = i < x.length ? x[i] : 0, q = i < y.length ? y[i] : 0;
    if (p != q) return p < q;
  }
  return false;
}

/// Предел памяти клиента: сообщений на чат, номеров в окне повторов, длина входящего текста (знаков) и блоба (байт).
const int maxStoredPerChat = 500, maxReplayIds = 5000, maxIncomingChars = 8000, maxIncomingBlob = 64 * 1024;

// ── Личность устройства ─────────────────────────────────────────────────────

/// Ключи этого устройства: Ed25519 (подпись) и X25519 (общий секрет). Случайные на сеанс, нигде не сохраняются.
class GuestIdentity {
  final SimpleKeyPair _ed, _x;
  final Uint8List edPub, xPub;
  GuestIdentity._(this._ed, this._x, this.edPub, this.xPub);

  static Future<GuestIdentity> generate() async {
    final ed = await Ed25519().newKeyPair();
    final x = await X25519().newKeyPair();
    return GuestIdentity._(ed, x, Uint8List.fromList((await ed.extractPublicKey()).bytes), Uint8List.fromList((await x.extractPublicKey()).bytes));
  }

  Future<Uint8List> sign(List<int> m) async => Uint8List.fromList((await Ed25519().sign(m, keyPair: _ed)).bytes);

  Future<Uint8List> ecdh(List<int> theirX) async {
    final s = await X25519().sharedSecretKey(keyPair: _x, remotePublicKey: SimplePublicKey(theirX, type: KeyPairType.x25519));
    return Uint8List.fromList(await s.extractBytes());
  }

  /// Подпись связывает Ed25519 и X25519 в одну связку (то же, что `keyBundleSignatureBase64` на телефоне).
  Future<String> keyBundleSignatureBase64() async => base64.encode(await sign([..._keysDomain, ...edPub, ...xPub]));

  static final _keysDomain = utf8.encode('YANDI-MOBILE-KEYS-V1\u0000');

  /// Проверка чужой связки: подпись сделана её же ключом Ed25519.
  static Future<bool> verifyKeyBundle({required List<int> ed, required List<int> x, required List<int> signature}) async {
    try {
      return await Ed25519().verify([..._keysDomain, ...ed, ...x], signature: Signature(signature, publicKey: SimplePublicKey(ed, type: KeyPairType.ed25519)));
    } catch (_) {
      return false;
    }
  }
}

class PeerKeyBundle {
  final List<int> x25519, ed25519;
  const PeerKeyBundle(this.x25519, this.ed25519);
}

// ── E4-конверт (e2e_crypto.dart телефона) ───────────────────────────────────

class E2e {
  static const headerSize = 134;
  static const ttlMs = 30 * 24 * 3600 * 1000;
  static final _domain = utf8.encode('YANDI-E2E-E4\u0000');
  static final _signDomain = utf8.encode('YANDI-E2E-E4-SIGN\u0000');

  static bool isEncrypted(List<int> d) => d.length >= headerSize + 16 + 64 && d[0] == 0xE4;

  static Future<SecretKey> _key(List<int> shared, List<int> header) async {
    if (shared.length != 32 || shared.every((b) => b == 0)) throw StateError('Invalid X25519 shared secret');
    return Hkdf(hmac: Hmac.sha256(), outputLength: 32).deriveKey(
      secretKey: SecretKey(shared),
      nonce: crypto.sha256.convert([..._domain, ...header]).bytes,
      info: utf8.encode('YANDI-E2E-E4-KEY'),
    );
  }

  static List<int> peerBytes(String hex) {
    if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(hex)) throw const FormatException('Invalid peer id');
    return unhex(hex);
  }

  static Future<List<int>> encrypt(String text, List<int> recipientPub,
      {required String from, required String to, required int kind, required Future<Uint8List> Function(List<int>) sign, int? timestampMs}) async {
    if (kind != 1 && kind != 2) throw ArgumentError('Invalid kind');
    final kp = await X25519().newKeyPair();
    final pub = await kp.extractPublicKey();
    final shared = await X25519().sharedSecretKey(keyPair: kp, remotePublicKey: SimplePublicKey(recipientPub, type: KeyPairType.x25519));
    final aes = AesGcm.with256bits();
    final nonce = aes.newNonce();
    final random = Random.secure();
    final ts = ByteData(8)..setUint64(0, timestampMs ?? DateTime.now().millisecondsSinceEpoch, Endian.big);
    final header = [0xE4, kind, ...peerBytes(from), ...peerBytes(to), ...List.generate(16, (_) => random.nextInt(256)), ...ts.buffer.asUint8List(), ...pub.bytes, ...nonce];
    final box = await aes.encrypt(utf8.encode(text), secretKey: await _key(await shared.extractBytes(), header), nonce: nonce, aad: [..._domain, ...header]);
    final wire = [...header, ...box.cipherText, ...box.mac.bytes];
    final signature = await sign([..._signDomain, ...wire]);
    if (signature.length != 64) throw StateError('Invalid signature');
    return [...wire, ...signature];
  }

  /// null — не E4, чужой адрес/вид, устарело, подпись не сошлась, не расшифровалось или повтор.
  static Future<String?> decrypt(Uint8List data, Future<Uint8List> Function(List<int>) ecdh,
      {required String from, required String to, required int kind, required List<int> senderEd, required bool Function(String id) claimReplay, void Function()? onDuplicate, int? nowMs}) async {
    if (!isEncrypted(data)) return null;
    try {
      if (data[1] != kind || (kind != 1 && kind != 2) || !_equal(peerBytes(from), data.sublist(2, 34)) || !_equal(peerBytes(to), data.sublist(34, 66))) return null;
      final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
      final ts = ByteData.sublistView(data, 82, 90).getUint64(0, Endian.big);
      if (ts > now + 300000 || now > ts + ttlMs) return null;
      final end = data.length - 64;
      if (!await Ed25519().verify([..._signDomain, ...data.sublist(0, end)], signature: Signature(data.sublist(end), publicKey: SimplePublicKey(senderEd, type: KeyPairType.ed25519)))) return null;
      final header = data.sublist(0, headerSize);
      final key = await _key(await ecdh(data.sublist(90, 122)), header);
      final plain = await AesGcm.with256bits().decrypt(
          SecretBox(data.sublist(headerSize, end - 16), nonce: data.sublist(122, 134), mac: Mac(data.sublist(end - 16, end))),
          secretKey: key, aad: [..._domain, ...header]);
      final text = utf8.decode(plain);
      final id = toHex(data.sublist(2, 82));
      if (!claimReplay(id)) {
        onDuplicate?.call();
        return null;
      }
      return text;
    } catch (_) {
      return null;
    }
  }

  static bool _equal(List<int> a, List<int> b) {
    if (a.length != b.length) return false;
    var d = 0;
    for (var i = 0; i < a.length; i++) {
      d |= a[i] ^ b[i];
    }
    return d == 0;
  }
}

// ── Служебные конверты внутри E4 (часть MsgChannel телефона: только то, что нужно гостю) ─

class MsgChannel {
  static const msgMarker = '\u0001yandi-msg:';
  static const rcptMarker = '\u0001yandi-rcpt:';

  /// Остальные служебные метки телефона (пинг поддержки, правка, реакция, картинка профиля): гость их не показывает как текст.
  static const _otherMarkers = ['\u0001yandi-av', '\u0001yandi-cap:', '\u0001yandi-edit:', '\u0001yandi-rx:'];

  static String wrapMessage(String cmid, String text) => msgMarker + jsonEncode({'v': 1, 'cmid': cmid, 'text': text});
  static String receipt(String kind, List<String> ids) => rcptMarker + jsonEncode({'kind': kind, 'ids': ids});

  static bool isOtherService(String t) => _otherMarkers.any(t.startsWith);

  /// Конверт сообщения: (cmid, text); null — это не конверт.
  static (String, String)? parseMessage(String t) {
    if (!t.startsWith(msgMarker)) return null;
    try {
      final m = jsonDecode(t.substring(msgMarker.length)) as Map<String, dynamic>;
      final cmid = m['cmid'] as String?, text = m['text'] as String?;
      if (cmid == null || text == null || cmid.isEmpty) return null;
      return (cmid, text);
    } catch (_) {
      return null;
    }
  }

  /// Квитанция: (kind, ids), kind — delivered или read.
  static (String, List<String>)? parseReceipt(String t) {
    if (!t.startsWith(rcptMarker)) return null;
    try {
      final m = jsonDecode(t.substring(rcptMarker.length)) as Map<String, dynamic>;
      final kind = m['kind'] as String?;
      final ids = (m['ids'] as List?)?.whereType<String>().toList();
      if (kind == null || ids == null || (kind != 'delivered' && kind != 'read')) return null;
      return (kind, ids);
    } catch (_) {
      return null;
    }
  }
}

/// Номер исходящего сообщения: время + 64 случайных бита (как `ChatMessage.generateId` на телефоне).
String newMessageId() {
  final rng = Random.secure();
  final rand = List.generate(8, (_) => rng.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  return '${DateTime.now().millisecondsSinceEpoch.toRadixString(16)}-$rand';
}

// ── TLS с закреплённым отпечатком ───────────────────────────────────────────

/// Клиент HTTPS, который доверяет ТОЛЬКО сертификату с этим отпечатком (SHA-256 от DER). Пустой отпечаток — ошибка, а не «доверять всем».
HttpClient pinnedHttpClient(String fingerprint) {
  final fp = fingerprint.trim().toLowerCase();
  if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(fp)) throw StateError('TLS certificate fingerprint is required');
  return HttpClient(context: SecurityContext(withTrustedRoots: false))
    ..connectionTimeout = const Duration(seconds: 10)
    ..badCertificateCallback = (X509Certificate cert, String host, int port) => crypto.sha256.convert(cert.der).toString().toLowerCase() == fp;
}

/// Предел ответа узла (как `LimitedClient` на телефоне): неисправный узел не заставит копить гигабайты.
const int maxResponseBytes = 8 * 1024 * 1024;

/// Адрес из частей; [host] может быть в скобках (`[::1]`) — скобки снимаются, `Uri` ставит их сам.
Uri _uri(String scheme, String host, int port, String path, [Map<String, String>? query]) {
  final h = host.startsWith('[') && host.endsWith(']') ? host.substring(1, host.length - 1) : host;
  return Uri(scheme: scheme, host: h, port: port, path: path, queryParameters: query);
}

/// Один запрос: код ответа и разобранный JSON (или null). Весь ответ — под одним сроком.
Future<(int, dynamic)> pinnedJson(HttpClient c, String method, Uri url, {String? token, Object? body, Duration timeout = const Duration(seconds: 15)}) async {
  return (() async {
    final req = await c.openUrl(method, url);
    if (token != null) req.headers.set('Authorization', 'Bearer $token');
    if (body != null) {
      req.headers.set('Content-Type', 'application/json');
      req.add(utf8.encode(jsonEncode(body)));
    }
    final res = await req.close();
    if (res.contentLength > maxResponseBytes) throw const HttpException('too large');
    final bytes = <int>[];
    await for (final chunk in res) {
      bytes.addAll(chunk);
      if (bytes.length > maxResponseBytes) throw const HttpException('too large');
    }
    dynamic parsed;
    try {
      parsed = bytes.isEmpty ? null : jsonDecode(utf8.decode(bytes));
    } catch (_) {
      parsed = null;
    }
    return (res.statusCode, parsed);
  })().timeout(timeout);
}

// ── Представление узла (node_hello.dart телефона) ───────────────────────────

/// Что подписывает узел, представляясь (`hello_message` на узле): метка, случайное число клиента, отпечаток сертификата в нижнем регистре.
List<int> helloMessage(List<int> nonce, String tlsFingerprintHex) => [...utf8.encode('YANDI-MOBILE-HELLO-V1\u0000'), ...nonce, ...utf8.encode(tlsFingerprintHex.toLowerCase())];

/// Убедиться, что по адресу из списка отвечает именно узел с номером и ключом из списка; возвращает ПРОВЕРЕННЫЙ отпечаток его сертификата.
/// Сертификат без проверки принимается только для этого запроса: дальше он сверяется с подписью узла, а затем закрепляется.
Future<String> verifyNodeHello(GuestNode n, {List<int>? nonce}) async {
  const bad = GuestError('Узел не подтвердил, что он тот самый из списка: вход отменён.');
  String? seen;
  final c = HttpClient(context: SecurityContext(withTrustedRoots: false))
    ..connectionTimeout = const Duration(seconds: 8)
    ..badCertificateCallback = (X509Certificate cert, String h, int p) {
      seen = crypto.sha256.convert(cert.der).toString();
      return true;
    };
  try {
    final nn = nonce ?? List<int>.generate(16, (_) => Random.secure().nextInt(256));
    final req = await c.getUrl(Uri(scheme: 'https', host: n.host, port: n.port, path: '/mobile/hello', queryParameters: {'nonce': toHex(nn)})).timeout(const Duration(seconds: 10));
    final res = await req.close().timeout(const Duration(seconds: 10));
    if (res.statusCode != 200) throw bad;
    final bytes = <int>[];
    await for (final chunk in res.timeout(const Duration(seconds: 10))) {
      bytes.addAll(chunk);
      if (bytes.length > 4096) throw bad;
    }
    final b = jsonDecode(utf8.decode(bytes));
    if (b is! Map<String, dynamic> || seen == null) throw bad;
    if ('${b['node_id']}'.toLowerCase() != n.id.toLowerCase() || '${b['key']}'.toLowerCase() != n.key.toLowerCase()) throw bad;
    final claimed = '${b['tls_fingerprint']}'.toLowerCase();
    if (claimed != seen!.toLowerCase()) throw bad; // подписан не тот сертификат, по которому подключились
    final sigHex = '${b['signature']}';
    if (!RegExp(r'^[0-9a-fA-F]{128}$').hasMatch(sigHex) || !RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(n.key)) throw bad;
    final ok = await Ed25519().verify(helloMessage(nn, claimed), signature: Signature(unhex(sigHex), publicKey: SimplePublicKey(unhex(n.key), type: KeyPairType.ed25519)));
    if (!ok) throw bad;
    return claimed;
  } on GuestError {
    rethrow;
  } catch (_) {
    throw const GuestError('Не удалось связаться с узлом. Проверьте интернет или выберите другой узел.');
  } finally {
    c.close(force: true);
  }
}

/// Код сопряжения `YANDI-PAIR-1:<base64 JSON>` (или старый открытый JSON): адрес, порт, одноразовый код и отпечаток сертификата.
class PairCode {
  final String host, code, fingerprint;
  final int port;
  const PairCode(this.host, this.port, this.code, this.fingerprint);

  /// null — текст не похож на код сопряжения или в нём нет отпечатка (без него сертификат проверить нельзя).
  static PairCode? parse(String raw) {
    try {
      var t = raw.trim();
      if (t.startsWith('YANDI-PAIR-1:')) t = utf8.decode(base64.decode(t.substring('YANDI-PAIR-1:'.length).trim()));
      final m = jsonDecode(t);
      if (m is! Map || m['invite'] == true) return null; // приглашения с PIN этот клиент не принимает
      final host = m['host'], port = m['port'], code = m['pairing_code'], fp = '${m['tls_fingerprint'] ?? ''}'.trim().toLowerCase();
      if (host is! String || port is! int || code is! String || !RegExp(r'^[0-9a-f]{64}$').hasMatch(fp)) return null;
      if (!validMobileAddr('${host.contains(':') && !host.startsWith('[') ? '[$host]' : host}:$port')) return null;
      return PairCode(host, port, code, fp);
    } catch (_) {
      return null;
    }
  }
}

/// Одно сообщение чата: входящее или исходящее. [status]: failed, pending, delivered, read.
class GuestMessage {
  final String id, peerId;
  /// Меняется один раз: сообщение собеседника, чьё устройство убрано с узла, становится надгробием ([tombstoneMarker]).
  String text;
  final bool outgoing;
  final DateTime time;
  String status;
  GuestMessage(this.id, this.peerId, this.text, this.outgoing, this.time, this.status);
}

/// Человек на узле из `GET /mobile/users`.
class GuestUser {
  final String peerId, name, role;
  final bool online, isNode;
  const GuestUser(this.peerId, this.name, this.role, this.online, this.isNode);

  static GuestUser? fromJson(Map<String, dynamic> j) {
    final id = j['peer_id'];
    if (id is! String || id.isEmpty) return null;
    final nick = '${j['nick'] ?? ''}'.trim(), name = '${j['name'] ?? ''}'.trim();
    return GuestUser(id, nick.isNotEmpty ? nick : (name.isNotEmpty ? name : id.substring(0, min(8, id.length))), '${j['role'] ?? ''}', j['online'] == true, j['is_node'] == true);
  }
}

// ── Клиент ──────────────────────────────────────────────────────────────────

/// Сеанс гостя на чужом узле. Создаётся через [enterGuest] или [enterWithCode], затем [start].
class GuestClient {
  final String host, nodeId;
  final int port;
  final String _token;
  final String fingerprint;
  final HttpClient _http;
  final GuestIdentity identity;

  String nodeName;
  String role = '';
  String? deviceId; // транспортный номер этого устройства (адрес в E4)
  String? nodePeerId;

  GuestClient._(this.host, this.port, this.nodeId, this.nodeName, this._token, this.fingerprint, this._http, this.identity);

  bool get isGuest => role == 'guest';

  /// Отзыв сборок: версия этого клиента ниже `min_app` (из `/mobile/info` или подписанного списка) — отправка запрещена, пока не обновят.
  String? requiredApp;
  bool get appRevoked => requiredApp != null;

  /// Принять `min_app` (узла или списка): строка новее нашей версии включает блокировку; уже включённая не снимается списком, только узлом.
  void noteMinApp(Object? v, {bool fromNode = true}) {
    final m = v is String && v.isNotEmpty && versionOlder(kGuestAppVersion, v) ? v : null;
    if (m != null) {
      requiredApp = m;
    } else if (fromNode) {
      requiredApp = null;
    }
  }

  static String deviceLabel() {
    final r = Random.secure();
    return 'ПК ${List.generate(2, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join()}';
  }

  /// Вход гостем без кода: узел из подписанного списка сначала представляется, затем `POST /mobile/pair/guest` по закреплённому сертификату.
  static Future<GuestClient> enterGuest(GuestNode n) async {
    final fp = n.key.isNotEmpty ? await verifyNodeHello(n) : n.fingerprint;
    if (fp.isEmpty) throw const GuestError('Не удалось проверить узел: вход гостем отменён.');
    final http = pinnedHttpClient(fp);
    try {
      final (code, body) = await pinnedJson(http, 'POST', _uri('https', n.host, n.port, '/mobile/pair/guest'), body: {'device_name': deviceLabel()}, timeout: const Duration(seconds: 10));
      if (code == 503) throw const GuestError('На этом узле сейчас нет места для гостей. Попробуйте другой узел или позже.');
      if (code == 403 || code == 404) throw const GuestError('Этот узел не принимает гостей. Попросите код или ссылку у администратора.');
      if (code == 429) throw const GuestError('Слишком много попыток. Подождите и повторите.');
      if (code != 200 || body is! Map) throw const GuestError('Узел отказал во входе гостем.');
      final token = body['token'];
      if (token is! String || token.isEmpty || body['role'] != 'guest') throw const GuestError('Узел ответил неверно: вход гостем отменён.');
      return await _open(n.host, n.port, n.id, n.name, token, fp, http);
    } on GuestError {
      http.close(force: true);
      rethrow;
    } catch (_) {
      http.close(force: true);
      throw const GuestError('Не удалось связаться с узлом. Проверьте интернет или выберите другой узел.');
    }
  }

  /// Вход по коду сопряжения (отпечаток сертификата берётся из кода). [host]/[port] — необязательная замена адреса из кода.
  static Future<GuestClient> enterWithCode(PairCode c, {String? host, int? port}) async {
    final h = host ?? c.host, p = port ?? c.port;
    final http = pinnedHttpClient(c.fingerprint);
    try {
      final (code, body) = await pinnedJson(http, 'POST', _uri('https', h, p, '/mobile/pair'),
          body: {'pairing_code': c.code, 'device_name': deviceLabel()}, timeout: const Duration(seconds: 10));
      if (code == 429) throw const GuestError('Слишком много попыток. Подождите и повторите.');
      if (code != 200 || body is! Map || body['token'] is! String || (body['token'] as String).isEmpty) {
        throw const GuestError('Код устарел или уже использован. Попросите у владельца узла новый.');
      }
      return await _open(h, p, '', h.contains(':') && !h.startsWith('[') ? '[$h]:$p' : '$h:$p', body['token'] as String, c.fingerprint, http);
    } on GuestError {
      http.close(force: true);
      rethrow;
    } catch (_) {
      http.close(force: true);
      throw const GuestError('Не удалось связаться с узлом. Проверьте интернет или выберите другой узел.');
    }
  }

  static Future<GuestClient> _open(String host, int port, String id, String name, String token, String fp, HttpClient http) async {
    final cl = GuestClient._(host, port, id, name, token, fp, http, await GuestIdentity.generate());
    final info = await cl.getInfo();
    if (info == null || '${info['device_id'] ?? ''}'.length != 64) throw const GuestError('Узел ответил неверно: вход гостем отменён.');
    return cl;
  }

  /// Адрес ручки узла собирается из частей: IPv6 получает скобки сам, строкой host не склеивается.
  Uri _url(String path, [Map<String, String>? query]) => _uri('https', host, port, path, query);

  Future<(int, dynamic)> _json(String method, String path, {Object? body, Duration timeout = const Duration(seconds: 15), Map<String, String>? query}) =>
      pinnedJson(_http, method, _url(path, query), token: _token, body: body, timeout: timeout);

  // ── ручки узла ────────────────────────────────────────────────────────────

  /// `GET /mobile/info`: роль, номер устройства и узла, имя. null — узел не ответил.
  Future<Map<String, dynamic>?> getInfo() async {
    try {
      final (c, b) = await _json('GET', '/mobile/info', timeout: const Duration(seconds: 10));
      if (c != 200 || b is! Map<String, dynamic>) return null;
      deviceId = b['device_id'] as String?;
      nodePeerId = b['node_id'] as String?;
      role = b['role'] as String? ?? role;
      noteMinApp(b['min_app']);
      final n = b['name'];
      if (n is String && n.isNotEmpty) nodeName = n;
      return b;
    } catch (_) {
      return null;
    }
  }

  /// Люди узла (в том числе строка самого компьютера, `is_node`). null — узел не ответил (это не «никого нет»).
  Future<List<GuestUser>?> getUsers() async {
    try {
      final (c, b) = await _json('GET', '/mobile/users', timeout: const Duration(seconds: 12));
      if (c != 200 || b is! Map) return null;
      return [for (final u in (b['users'] as List? ?? const [])) if (u is Map) ?GuestUser.fromJson(u.cast<String, dynamic>())];
    } catch (_) {
      return null;
    }
  }

  /// Каталог «Узлы»: онлайн-узлы без адресов [{short_id, region, role, online}].
  Future<List<Map<String, dynamic>>?> getNodes() async {
    try {
      final (c, b) = await _json('GET', '/mobile/nodes', timeout: const Duration(seconds: 12));
      if (c != 200 || b is! Map) return null;
      return [for (final n in (b['nodes'] as List? ?? const [])) if (n is Map) n.cast<String, dynamic>()];
    } catch (_) {
      return null;
    }
  }

  /// Жалоба на человека (`user`, номер устройства). null — отправлено, иначе текст причины.
  Future<String?> submitComplaint({required String targetId, required String reason, required String text}) async {
    try {
      final (c, b) = await _json('POST', '/mobile/complaints', body: {'target_kind': 'user', 'target_id': targetId, 'reason': reason, 'text': text, 'evidence': const <String>[]});
      if (c == 200) return null;
      if (b is Map && b['error'] is String) return b['error'] as String;
      return 'Не удалось отправить жалобу';
    } catch (_) {
      return 'Узел не отвечает: жалоба не отправлена';
    }
  }

  /// Гость просит администратора дать ему статус пользователя (текст до 300 знаков). null — отправлено, иначе готовая фраза.
  Future<String?> requestUserStatus(String text) async {
    try {
      final (c, _) = await _json('POST', '/mobile/pair/request', body: {'text': text}, timeout: const Duration(seconds: 10));
      if (c == 200) return null;
      if (c == 400) return 'В обращении не должно быть ссылок.';
      if (c == 429) return 'У администратора сейчас слишком много запросов. Повторите позже.';
      return 'Не удалось отправить запрос. Попробуйте позже.';
    } catch (_) {
      return 'Не удалось отправить запрос. Попробуйте позже.';
    }
  }

  /// Что с запросом статуса: none / pending / accepted / rejected.
  Future<String> getStatusRequest() async {
    try {
      final (c, b) = await _json('GET', '/mobile/pair/request', timeout: const Duration(seconds: 10));
      return c == 200 && b is Map ? (b['state'] as String? ?? 'none') : 'none';
    } catch (_) {
      return 'none';
    }
  }

  /// Ник этого устройства (виден другим в списке людей).
  Future<bool> putNick(String nick) async {
    try {
      final (c, _) = await _json('PUT', '/mobile/me/profile', body: {'nick': nick});
      return c == 200;
    } catch (_) {
      return false;
    }
  }

  /// Выйти: токен отзывается (лучшее усилие, как `revokeSelf` на телефоне).
  Future<void> _revokeSelf() async {
    try {
      await _json('DELETE', '/mobile/me', timeout: const Duration(seconds: 8));
    } catch (_) {}
  }

  // ── ключи ─────────────────────────────────────────────────────────────────

  Future<void> registerPublicKeys() async {
    final (c, _) = await _json('POST', '/mobile/pubkeys',
        body: {'ed25519_pub': base64.encode(identity.edPub), 'x25519_pub': base64.encode(identity.xPub), 'signature': await identity.keyBundleSignatureBase64()}, timeout: const Duration(seconds: 10));
    if (c != 200) throw const GuestError('Узел не сохранил связку ключей этого устройства.');
  }

  /// Закреплённые на сеанс подписи собеседников: смена ключа посреди сеанса — отказ (как PeerTrust на телефоне).
  final Map<String, String> _pins = {};
  final Map<String, Future<PeerKeyBundle?>> _bundleInFlight = {};

  /// Подписанная связка ключей собеседника с проверкой подписи и закреплением Ed25519.
  Future<PeerKeyBundle?> getPeerBundle(String peerId) {
    final running = _bundleInFlight[peerId];
    if (running != null) return running;
    final f = _fetchPeerBundle(peerId);
    _bundleInFlight[peerId] = f;
    f.whenComplete(() {
      if (identical(_bundleInFlight[peerId], f)) _bundleInFlight.remove(peerId);
    });
    return f;
  }

  Future<PeerKeyBundle?> _fetchPeerBundle(String peerId) async {
    try {
      if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(peerId)) return null;
      final (c, b) = await _json('GET', '/mobile/pubkey/$peerId', timeout: const Duration(seconds: 8));
      if (c != 200 || b is! Map) return null;
      final pubB64 = b['x25519_pub'] as String?, edB64 = b['ed25519_pub'] as String?, sigB64 = b['signature'] as String?;
      if (pubB64 == null || edB64 == null || sigB64 == null) return null;
      final pub = base64.decode(pubB64), ed = base64.decode(edB64), sig = base64.decode(sigB64);
      if (pub.length != 32 || ed.length != 32 || sig.length != 64) return null;
      if (!await GuestIdentity.verifyKeyBundle(ed: ed, x: pub, signature: sig)) return null;
      final old = _pins[peerId];
      if (old != null && old != edB64) {
        _securityCtrl.add('Ключ собеседника изменился. Отправка запрещена: проверьте отпечаток вне YANDI.');
        return null;
      }
      _pins[peerId] = edB64;
      return PeerKeyBundle(pub, ed);
    } catch (_) {
      return null;
    }
  }

  // ── сообщения ─────────────────────────────────────────────────────────────

  final _msgCtrl = StreamController<GuestMessage>.broadcast();
  final _receiptCtrl = StreamController<(String, List<String>)>.broadcast(); // (kind, ids)
  final _statusCtrl = StreamController<(String, bool)>.broadcast();
  final _offlineCtrl = StreamController<String>.broadcast();
  final _securityCtrl = StreamController<String>.broadcast();
  final _connCtrl = StreamController<bool>.broadcast();
  final _forgottenCtrl = StreamController<void>.broadcast();

  /// Входящие сообщения (уже расшифрованные, без служебных конвертов).
  Stream<GuestMessage> get messages => _msgCtrl.stream;
  /// Квитанции собеседников: (delivered|read, номера моих сообщений).
  Stream<(String, List<String>)> get receipts => _receiptCtrl.stream;
  Stream<(String, bool)> get peerStatus => _statusCtrl.stream;
  /// Номер устройства, которому не доставлено: его нет на связи.
  Stream<String> get peerOffline => _offlineCtrl.stream;
  /// Сообщения собеседников, убранных с узла, стали надгробиями: экрану перерисоваться.
  Stream<void> get forgotten => _forgottenCtrl.stream;
  Stream<String> get securityErrors => _securityCtrl.stream;
  Stream<bool> get connection => _connCtrl.stream;

  /// Окно повторов: ограниченное (самые давние номера вытесняются), чтобы поток писем не раздувал память.
  final Set<String> _seenReplay = <String>{};
  bool _claimReplay(String id) {
    if (_seenReplay.contains(id)) return false;
    _seenReplay.add(id);
    if (_seenReplay.length > maxReplayIds) _seenReplay.remove(_seenReplay.first);
    return true;
  }

  /// Хранилище переписки живёт в клиенте, а не в экране: письма, пришедшие до появления экрана, не теряются (экран читает его при открытии).
  final Map<String, List<GuestMessage>> _store = {};
  final Set<String> _unread = {};
  final Set<String> _readSent = {};

  /// Сообщения чата с [peerId] (не более [maxStoredPerChat] последних), по времени получения.
  List<GuestMessage> messagesFor(String peerId) => List.unmodifiable(_store[peerId] ?? const <GuestMessage>[]);
  /// Собеседники с непрочитанным.
  Set<String> get unread => Set.unmodifiable(_unread);

  void _remember(GuestMessage m) {
    final l = _store.putIfAbsent(m.peerId, () => []);
    if (l.any((x) => x.id == m.id && x.outgoing == m.outgoing)) return;
    l.add(m);
    if (l.length > maxStoredPerChat) l.removeRange(0, l.length - maxStoredPerChat);
  }

  /// Чат открыт: непрочитанное снимается, собеседнику уходит «прочитано» по ещё не отмеченным входящим.
  void markRead(String peer) {
    _unread.remove(peer);
    final ids = [for (final m in _store[peer] ?? const <GuestMessage>[]) if (!m.outgoing && _readSent.add(m.id)) m.id];
    if (ids.length > 200) ids.removeRange(200, ids.length);
    if (ids.isNotEmpty) unawaited(sendReceipt(peer, 'read', ids));
  }

  // ── устройства, убранные с узла: их сообщения — надгробия (только в памяти, как и вся переписка гостя) ──

  int _forgottenCursor = 0;
  bool _forgottenBusy = false, _forgottenAgain = false;

  /// Забрать `GET /mobile/forgotten?after=курсор` и превратить входящие от этих устройств в надгробия. Зовётся при открытии сокета и по кадру 0x17;
  /// параллельные вызовы сливаются в повторный проход.
  Future<void> syncForgotten() async {
    if (_closed) return;
    if (_forgottenBusy) { _forgottenAgain = true; return; }
    _forgottenBusy = true;
    try {
      do {
        _forgottenAgain = false;
        for (var page = 0; page < 50 && !_closed; page++) {
          final (c, b) = await _json('GET', '/mobile/forgotten', query: {'after': '$_forgottenCursor'}, timeout: const Duration(seconds: 10));
          if (c != 200 || b is! Map) break;
          final items = b['items'] is List ? (b['items'] as List) : const [];
          var next = _forgottenCursor;
          for (final it in items) {
            if (it is! Map) continue;
            final n = it['n'], peer = it['peer'];
            if (n is! num) continue;
            if (peer is String && RegExp(r'^[0-9a-f]{64}$').hasMatch(peer)) _forgetPeer(peer);
            if (n.toInt() > next) next = n.toInt();
          }
          final cur = b['cursor'];
          if (cur is num && cur.toInt() > next) next = cur.toInt();
          if (next <= _forgottenCursor) break;
          _forgottenCursor = next;
          if (items.isEmpty) break;
        }
      } while (_forgottenAgain && !_closed);
    } catch (_) {
      // курсор не сдвинут: следующий проход повторит
    } finally {
      _forgottenBusy = false;
    }
  }

  void _forgetPeer(String peer) {
    for (final m in _store[peer] ?? const <GuestMessage>[]) {
      if (!m.outgoing) m.text = tombstoneMarker;
    }
    _unread.remove(peer);
    if (!_forgottenCtrl.isClosed) _forgottenCtrl.add(null);
  }

  void _applyReceipt(String kind, List<String> ids) {
    int rk(String s) => switch (s) { 'read' => 2, 'delivered' => 1, _ => 0 };
    final rank = rk(kind);
    final set = ids.length > 500 ? ids.sublist(0, 500).toSet() : ids.toSet();
    for (final l in _store.values) {
      for (final m in l) {
        if (m.outgoing && set.contains(m.id) && rk(m.status) < rank) m.status = kind;
      }
    }
  }

  /// Времена отправок за скользящие сутки. Владеет ими клиент: лимит проверяется в самой отправке, а не только в экране.
  final List<DateTime> _sentAt = [];

  /// Сколько сообщений ещё можно отправить за скользящие сутки; не гость — без лимита (-1). Узел держит и свой потолок.
  int get messagesLeft {
    if (!isGuest) return -1;
    final since = DateTime.now().subtract(const Duration(hours: 24));
    _sentAt.removeWhere((t) => !t.isAfter(since));
    return (guestMessagesPerDay - _sentAt.length).clamp(0, guestMessagesPerDay);
  }

  /// Отправить текст зашифрованно. Возвращает сообщение (status pending) или null: нет сокета, ключа собеседника, лимит исчерпан или сборка отозвана.
  /// Открытым текстом никогда не отправляет. Место в лимите занимается ДО отправки (параллельные вызовы не обходят лимит) и возвращается,
  /// если до сокета дело не дошло.
  Future<GuestMessage?> sendText(String peerId, String text) async {
    if (appRevoked || _closed) return null;
    if (text.isEmpty || text.length > 2000) return null;
    final guest = isGuest;
    if (guest && messagesLeft == 0) return null;
    final now = DateTime.now();
    if (guest) _sentAt.add(now);
    final id = newMessageId();
    if (!await _sendEncrypted(peerId, MsgChannel.wrapMessage(id, text))) {
      if (guest) _sentAt.remove(now);
      return null;
    }
    final msg = GuestMessage(id, peerId, text, true, now, 'pending');
    _remember(msg);
    return msg;
  }

  Future<bool> _sendEncrypted(String peerId, String payloadText) async {
    final ch = _ws;
    final self = deviceId;
    if (ch == null || self == null || !RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(peerId)) return false;
    try {
      final bundle = await getPeerBundle(peerId);
      if (bundle == null) return false;
      final blob = await E2e.encrypt(payloadText, bundle.x25519, from: self, to: peerId, kind: 1, sign: identity.sign);
      final buf = BytesBuilder()
        ..addByte(ftSendMsg)
        ..add(E2e.peerBytes(peerId))
        ..add((ByteData(4)..setUint32(0, blob.length, Endian.little)).buffer.asUint8List())
        ..add(blob);
      ch.add(buf.toBytes());
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Квитанция «прочитано»/«доставлено» собеседнику (без неё у него не появятся галочки). Служебное: в лимит не входит.
  Future<void> sendReceipt(String peerId, String kind, List<String> ids) async {
    if (ids.isEmpty) return;
    await _sendEncrypted(peerId, MsgChannel.receipt(kind, ids));
  }

  /// Расшифровать входящий блоб (сокет и почта): только E4 текущего формата, повторы отбрасываются. null — показывать нечего.
  Future<String?> _decryptIncoming(List<int> payload, String from, {void Function()? onDuplicate}) async {
    final self = deviceId;
    if (self == null || !E2e.isEncrypted(payload) || !RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(from)) return null;
    final bundle = await getPeerBundle(from);
    if (bundle == null) return null;
    return E2e.decrypt(Uint8List.fromList(payload), identity.ecdh, from: from, to: self, kind: 1, senderEd: bundle.ed25519, claimReplay: _claimReplay, onDuplicate: onDuplicate);
  }

  /// Разобрать расшифрованный текст: квитанция уходит в поток, служебное пропускается, конверт или обычный текст — сообщение.
  void _deliver(String from, DateTime ts, String text) {
    final rcpt = MsgChannel.parseReceipt(text);
    if (rcpt != null) {
      _applyReceipt(rcpt.$1, rcpt.$2);
      _receiptCtrl.add(rcpt);
      return;
    }
    if (text.startsWith(MsgChannel.rcptMarker) || MsgChannel.isOtherService(text)) return;
    final env = MsgChannel.parseMessage(text);
    if (text.startsWith(MsgChannel.msgMarker) && env == null) return; // повреждённый конверт не показываем как текст
    final shown = env?.$2 ?? text;
    if (shown.length > maxIncomingChars) return; // слишком длинный текст не храним и не показываем
    final msg = GuestMessage(env?.$1 ?? 'in_${ts.millisecondsSinceEpoch}_${from.substring(0, 8)}', from, shown, false, ts, 'delivered');
    _remember(msg);
    _unread.add(from);
    _msgCtrl.add(msg);
    if (env != null) unawaited(sendReceipt(from, 'delivered', [env.$1]));
  }

  // ── сокет ─────────────────────────────────────────────────────────────────

  WebSocket? _ws;
  Timer? _ping, _reconnect, _inboxTimer;
  int _delay = 5;
  bool _closed = false;

  /// Зарегистрировать ключи, открыть сокет и забрать накопившуюся почту.
  Future<void> start() async {
    await registerPublicKeys();
    _connect();
    unawaited(syncInbox());
    _inboxTimer = Timer.periodic(const Duration(seconds: 30), (_) => syncInbox());
  }

  void _connect() async {
    if (_closed) return;
    try {
      final ws = await WebSocket.connect(
        _uri('wss', host, port, '/mobile/ws').toString(),
        headers: {'Authorization': 'Bearer $_token'}, // токен в заголовке, не в адресе
        customClient: pinnedHttpClient(fingerprint),
      );
      if (_closed) {
        await ws.close();
        return;
      }
      _ws = ws;
      _delay = 5;
      _connCtrl.add(true);
      _ping?.cancel();
      _ping = Timer.periodic(const Duration(seconds: 30), (_) => _rawSend(Uint8List.fromList([ftPing])));
      ws.listen(_onData, onError: (_) => _lost(ws), onDone: () => _lost(ws), cancelOnError: true);
      unawaited(syncInbox());
      unawaited(syncForgotten());
    } catch (_) {
      _scheduleReconnect();
    }
  }

  void _lost(WebSocket ws) {
    if (!identical(_ws, ws)) return;
    _ws = null;
    _ping?.cancel();
    _connCtrl.add(false);
    _scheduleReconnect();
  }

  void _scheduleReconnect() {
    if (_closed) return;
    _reconnect?.cancel();
    _reconnect = Timer(Duration(seconds: _delay), _connect);
    _delay = (_delay * 2).clamp(5, 60);
  }

  bool get connected => _ws != null;

  void _rawSend(Uint8List b) {
    try {
      _ws?.add(b);
    } catch (_) {}
  }

  void _onData(dynamic raw) async {
    if (raw is! List<int> || raw.isEmpty) return;
    final d = Uint8List.fromList(raw);
    switch (d[0]) {
      case ftPing:
        _rawSend(Uint8List.fromList([ftPong]));
      case ftChatMsg:
        await _onChat(d);
      case ftPeerStatus:
        if (d.length >= 34) _statusCtrl.add((toHex(d.sublist(1, 33)), d[33] != 0));
      case ftPeerOffline:
        if (d.length >= 33) _offlineCtrl.add(toHex(d.sublist(1, 33)));
      case ftForgotten:
        if (d.length >= 9) unawaited(syncForgotten());   // только намёк: список забирается запросом с курсором
    }
  }

  static const _maxDateMs = 8640000000000000;

  /// `[0x10][32 от кого][8 время мс LE][4 длина LE][блоб]`.
  Future<void> _onChat(Uint8List d) async {
    if (d.length < 45) return;
    final from = toHex(d.sublist(1, 33));
    final ts = ByteData.sublistView(d, 33, 41).getInt64(0, Endian.little);
    final len = ByteData.sublistView(d, 41, 45).getUint32(0, Endian.little);
    if (d.length < 45 + len || ts < 0 || ts > _maxDateMs) return;
    if (len > maxIncomingBlob) return;
    final text = await _decryptIncoming(d.sublist(45, 45 + len), from);
    if (text != null) _deliver(from, DateTime.fromMillisecondsSinceEpoch(ts), text);
  }

  // ── почта ─────────────────────────────────────────────────────────────────

  bool _syncing = false;

  /// Забрать неподтверждённую почту страницами (`after` — номер последнего письма страницы, как на телефоне), показать новое и подтвердить
  /// обработанное. Письма, которые не расшифровались из-за нехватки ключей или сети, остаются на узле и не загораживают очередь: следующая
  /// страница берётся по курсору. Подтверждаются повторы и то, что не может стать сообщением никогда (не конверт E4, битое или слишком большое);
  /// старый формат E2/E3 не подтверждается и не стирается (как на телефоне).
  Future<void> syncInbox() async {
    if (_syncing || _closed || deviceId == null) return;
    _syncing = true;
    try {
      const page = 200, maxPages = 20;
      var after = 0;
      for (var i = 0; i < maxPages && !_closed; i++) {
        final (c, b) = await _json('GET', '/mobile/inbox', query: {'since': '0', 'after': '$after', 'limit': '$page'});
        if (c != 200 || b is! Map) return;
        final list = (b['messages'] as List? ?? const []).whereType<Map>().toList();
        final ack = <int>[];
        var last = after;
        for (final m in list) {
          final id = (m['id'] as num?)?.toInt();
          if (id == null) continue;
          if (id > last) last = id;
          final from = m['from_peer_id'] as String? ?? '', p64 = m['payload_b64'] as String? ?? '';
          final ts = (m['ts_ms'] as num?)?.toInt() ?? 0;
          if (from.isEmpty || p64.isEmpty || ts < 0 || ts > _maxDateMs) {
            ack.add(id); // письмо без отправителя, тела или с невозможным временем не станет сообщением никогда
            continue;
          }
          if (p64.length > maxIncomingBlob * 2) {
            ack.add(id); // заведомо больше любого письма: мусор, подтверждаем
            continue;
          }
          Uint8List bytes;
          try {
            bytes = base64.decode(p64);
          } catch (_) {
            ack.add(id); // не base64: не станет письмом никогда
            continue;
          }
          if (bytes.isNotEmpty && (bytes[0] == 0xE2 || bytes[0] == 0xE3)) continue; // старый формат не подтверждаем и не стираем
          if (bytes.length > maxIncomingBlob || !E2e.isEncrypted(bytes)) {
            ack.add(id); // не E4 и не старый формат: структурный мусор
            continue;
          }
          var dup = false;
          final text = await _decryptIncoming(bytes, from, onDuplicate: () => dup = true);
          if (text == null) {
            if (dup) ack.add(id); // повтор того, что уже получено вживую
            continue; // нет ключа/сети/подпись не сошлась: остаётся на узле, но очередь идёт дальше по курсору
          }
          _deliver(from, DateTime.fromMillisecondsSinceEpoch(ts), text); // сначала в хранилище клиента, потом подтверждение
          ack.add(id);
        }
        if (ack.isNotEmpty) await _json('POST', '/mobile/inbox/ack', body: {'ids': ack}, timeout: const Duration(seconds: 10));
        if (list.length < page || last <= after) break;
        after = last;
      }
    } catch (_) {
      // узел не ответил: повторим на следующем проходе
    } finally {
      _syncing = false;
    }
  }

  /// Закрыть сеанс: сокет, таймеры и (лучшим усилием) токен на узле.
  Future<void> close({bool revoke = true}) async {
    if (_closed) return;
    if (revoke) await _revokeSelf();
    _closed = true;
    _ping?.cancel();
    _reconnect?.cancel();
    _inboxTimer?.cancel();
    try {
      await _ws?.close();
    } catch (_) {}
    _ws = null;
    _http.close(force: true);
    for (final c in [_msgCtrl, _receiptCtrl, _statusCtrl, _offlineCtrl, _securityCtrl, _connCtrl, _forgottenCtrl]) {
      unawaited(c.close());
    }
  }
}
