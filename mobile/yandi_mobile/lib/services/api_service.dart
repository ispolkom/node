import 'dart:typed_data';
import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart' as crypto;
import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import '../models/trusted_node.dart';
import '../models/contact.dart';
import '../models/message.dart';
import '../crypto/identity.dart';
import '../crypto/peer_trust.dart';
import 'storage_service.dart';

class ApiService {
  TrustedNode _node;
  late http.Client _client;
  String? _deviceId;
  final void Function(String)? onSecurityError;
  final _trust = PeerTrust(read: StorageService.secureRead,write: StorageService.secureWrite);

  ApiService(this._node, {this.onSecurityError}) {
    _client = _buildClient(_node.fingerprint);
  }

  void switchNode(TrustedNode node) {
    _client.close();
    _node   = node;
    _deviceId = null;
    _client = _buildClient(node.fingerprint);
  }

  Map<String, String> get _headers => {
    'Content-Type':  'application/json',
    'Authorization': 'Bearer ${_node.token ?? ""}',
  };

  Uri _url(String path) => Uri.parse('${_node.httpUrl}$path');

  Future<Map<String, dynamic>> getInfo() async {
    final res = await _client
        .get(_url('/mobile/info'), headers: _headers)
        .timeout(const Duration(seconds: 10));
    if (res.statusCode != 200) throw StateError('Node info unavailable');
    final info = jsonDecode(res.body) as Map<String, dynamic>;
    _deviceId = info['device_id'] as String?;
    return info;
  }

  /// Единый профиль с узла: {nick, avatar_hash, updated_ms}; null — узел не ответил (старый узел без профиля тоже).
  Future<Map<String, dynamic>?> getProfile() async {
    try {
      final res = await _client.get(_url('/mobile/profile'), headers: _headers).timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) return null;
      return jsonDecode(res.body) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  /// Записать профиль на узел: [nick] и/или [avatarB64] (пусто — убрать картинку). Возвращает профиль, как его теперь видит узел.
  Future<Map<String, dynamic>?> putProfile({String? nick, String? avatarB64}) async {
    try {
      final res = await _client
          .put(_url('/mobile/profile'), headers: _headers, body: jsonEncode({if (nick != null) 'nick': nick, if (avatarB64 != null) 'avatar_b64': avatarB64}))
          .timeout(const Duration(seconds: 15));
      if (res.statusCode != 200) return null;
      return jsonDecode(res.body) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  Future<Uint8List?> getProfileAvatar() async {
    try {
      final res = await _client.get(_url('/mobile/profile/avatar'), headers: _headers).timeout(const Duration(seconds: 15));
      return res.statusCode == 200 ? res.bodyBytes : null;
    } catch (_) {
      return null;
    }
  }

  Future<String?> devicePeerId() async {
    if (_deviceId == null) await getInfo();
    return _deviceId;
  }

  /// Временные учётные данные сервера звонков (TURN) или null, если он на узле не настроен.
  Future<Map<String, dynamic>?> getTurn() async {
    try {
      final res = await _client
          .get(_url('/mobile/turn'), headers: _headers)
          .timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) return null;
      return jsonDecode(res.body) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  Future<List<Contact>> getContacts() async {
    final res = await _client
        .get(_url('/mobile/contacts'), headers: _headers)
        .timeout(const Duration(seconds: 10));
    final data = jsonDecode(res.body) as Map<String, dynamic>;
    final list = data['contacts'] as List<dynamic>? ?? [];
    return list.map((e) {
      final m = e as Map<String, dynamic>;
      // своё имя контакта сильнее его ника; ник подставляется, только когда имя не задано (пусто или обрезок номера)
      final name = m['display_name'] as String? ?? '';
      final nick = (m['nick'] as String?)?.trim() ?? '';
      final unnamed = name.isEmpty || RegExp(r'^[0-9a-f]{8,16}$').hasMatch(name);
      return Contact(
        peerId:      m['peer_id']      as String,
        displayName: (unnamed && nick.isNotEmpty) ? nick : name,
        online:      m['online']       as bool?   ?? false,
        isManual:    m['is_manual']    as bool?   ?? false,
        lastActivityMs: (m['last_activity_ms'] as num?)?.toInt() ?? 0,
      );
    }).toList();
  }

  Future<List<ChatMessage>> getHistory(String peerId, String myPeerId,
      {int limit = 50}) async {
    try {
      final res = await _client
          .get(_url('/mobile/chat/$peerId?limit=$limit'), headers: _headers)
          .timeout(const Duration(seconds: 10));
      final data = jsonDecode(res.body) as Map<String, dynamic>;
      final list = data['messages'] as List<dynamic>? ?? [];
      return list.map((e) {
        final m = e as Map<String, dynamic>;
        return ChatMessage(
          id:        m['id']        as String,
          peerId:    peerId,
          outgoing:  m['from_peer_id'] == myPeerId,
          text:      m['text']      as String,
          timestamp: DateTime.fromMillisecondsSinceEpoch(m['ts_ms'] as int),
          status:    MessageStatus.delivered,
        );
      }).toList();
    } catch (_) {
      return [];
    }
  }

  Future<Map<String, dynamic>> getProxyInfo() async {
    try {
      final res = await _client
          .get(_url('/mobile/proxy/info'), headers: _headers)
          .timeout(const Duration(seconds: 5));
      return jsonDecode(res.body) as Map<String, dynamic>;
    } catch (_) {
      return {};
    }
  }

  Future<List<int>?> getPeerX25519Pub(String peerId) async {
    return (await getPeerBundle(peerId))?.x25519;
  }

  Future<PeerKeyBundle?> getPeerBundle(String peerId) async {
    try {
      final res = await _client
          .get(_url('/mobile/pubkey/$peerId'), headers: _headers)
          .timeout(const Duration(seconds: 8));
      if (res.statusCode != 200) return null;
      final bundle = jsonDecode(res.body) as Map<String, dynamic>;
      final pubB64 = bundle['x25519_pub'] as String?;
      final edB64 = bundle['ed25519_pub'] as String?;
      final sigB64 = bundle['signature'] as String?;
      if (pubB64 == null || edB64 == null || sigB64 == null) return null;
      final pub = base64.decode(pubB64);
      final ed = base64.decode(edB64);
      final sig = base64.decode(sigB64);
      if (pub.length != 32 || ed.length != 32 || sig.length != 64) return null;
      if (!await Identity.verifyKeyBundle(
        ed25519PublicKey: ed,
        x25519PublicKey: pub,
        signature: sig,
      )) return null;
      await _trust.accept(peerId,ed);
      await StorageService.cachePeerX25519Pub(peerId, pubB64);
      return PeerKeyBundle(pub,ed);
    } on StateError catch (e) {
      onSecurityError?.call(e.toString());
      return null;
    } catch (_) {
      return null;
    }
  }

  Future<void> registerPublicKeys({
    required String ed25519PubBase64,
    required String x25519PubBase64,
    required String signatureBase64,
  }) async {
    try {
      final res = await _client.post(
        _url('/mobile/pubkeys'),
        headers: _headers,
        body: jsonEncode({
          'ed25519_pub': ed25519PubBase64,
          'x25519_pub': x25519PubBase64,
          'signature': signatureBase64,
        }),
      ).timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) {
        throw StateError(res.statusCode == 409
          ? 'Ключ этого устройства изменился: нужно новое сопряжение после проверки владельцем.'
          : 'Узел не сохранил связку ключей (${res.statusCode}).');
      }
    } on StateError catch (e) {
      onSecurityError?.call(e.toString());
      rethrow;
    }
  }

  /// Вся неподтверждённая почта, страницами (`after` — номер последнего письма страницы): письма, которые приложение не может принять
  /// (старый формат), остаются на узле, но не загораживают новые.
  Future<List<Map<String, dynamic>>> fetchInbox(int sinceMs) async {
    const page = 1000, maxPages = 20;
    final out = <Map<String, dynamic>>[];
    var after = 0;
    try {
      for (var i = 0; i < maxPages; i++) {
        final res = await _client
            .get(_url('/mobile/inbox?since=$sinceMs&after=$after&limit=$page'), headers: _headers)
            .timeout(const Duration(seconds: 15));
        if (res.statusCode != 200) break;
        final data = jsonDecode(res.body) as Map<String, dynamic>;
        final list = (data['messages'] as List<dynamic>?)?.cast<Map<String, dynamic>>() ?? const <Map<String, dynamic>>[];
        out.addAll(list);
        if (list.length < page) break;
        final last = list.last['id'];
        if (last is! num || last.toInt() <= after) break;
        after = last.toInt();
      }
    } catch (_) {}
    return out;
  }

  Future<void> ackInbox(List<int> ids) async {
    if (ids.isEmpty) return;
    try {
      await _client.post(
        _url('/mobile/inbox/ack'),
        headers: _headers,
        body: jsonEncode({'ids': ids}),
      ).timeout(const Duration(seconds: 10));
    } catch (_) {}
  }

  Future<String?> startFileTransfer({
    required String toPeerId,
    required String fileName,
    required int    fileSize,
    required int    totalChunks,
  }) async {
    try {
      final res = await _client.post(
        _url('/mobile/files'),
        headers: _headers,
        body: jsonEncode({'to_peer_id': toPeerId, 'file_name': fileName,
                          'file_size': fileSize, 'total_chunks': totalChunks}),
      ).timeout(const Duration(seconds: 15));
      if (res.statusCode != 200) return null;
      return (jsonDecode(res.body) as Map<String, dynamic>)['transfer_id'] as String?;
    } catch (_) {
      return null;
    }
  }

  Future<bool> uploadChunk(String transferId, int idx, List<int> bytes) async {
    try {
      final res = await _client.put(
        _url('/mobile/files/$transferId/chunk/$idx'),
        headers: {'Authorization': 'Bearer ${_node.token ?? ""}',
                  'Content-Type': 'application/octet-stream'},
        body: bytes,
      ).timeout(const Duration(seconds: 60));
      return res.statusCode == 200;
    } catch (_) {
      return false;
    }
  }

  Future<bool> completeTransfer(String transferId) async {
    try {
      final res = await _client.post(
        _url('/mobile/files/$transferId/done'), headers: _headers)
          .timeout(const Duration(seconds: 15));
      return res.statusCode == 200;
    } catch (_) {
      return false;
    }
  }

  Future<List<Map<String, dynamic>>> listFiles() async {
    try {
      final res = await _client
          .get(_url('/mobile/files'), headers: _headers)
          .timeout(const Duration(seconds: 10));
      if (res.statusCode != 200) return [];
      return ((jsonDecode(res.body) as Map<String, dynamic>)['files'] as List<dynamic>?)
             ?.cast<Map<String, dynamic>>() ?? [];
    } catch (_) {
      return [];
    }
  }

  Future<List<int>?> downloadChunk(String transferId, int idx) async {
    try {
      final res = await _client
          .get(_url('/mobile/files/$transferId/chunk/$idx'), headers: _headers)
          .timeout(const Duration(seconds: 60));
      if (res.statusCode != 200) return null;
      return res.bodyBytes;
    } catch (_) {
      return null;
    }
  }

  Future<bool> deleteTransfer(String transferId) async {
    try {
      final req = http.Request('DELETE', _url('/mobile/files/$transferId'));
      _headers.forEach((k, v) => req.headers[k] = v);
      final res = await _client.send(req).timeout(const Duration(seconds: 10));
      return res.statusCode == 200;
    } catch (_) {
      return false;
    }
  }

  /// Двоичный запрос (куски файлов групп): (код ответа, тело).
  Future<(int, List<int>)> bytes(String method, String path, [List<int>? body]) async {
    final req = http.Request(method, _url(path));
    req.headers['Authorization'] = 'Bearer ${_node.token ?? ""}';
    if (body != null) { req.headers['Content-Type'] = 'application/octet-stream'; req.bodyBytes = body; }
    final res = await http.Response.fromStream(await _client.send(req).timeout(const Duration(seconds: 60)));
    return (res.statusCode, res.bodyBytes);
  }

  /// Общий JSON-запрос (группы и др.): (код ответа, разобранное тело).
  Future<(int, dynamic)> json(String method, String path, [Object? body]) async {
    final req = http.Request(method, _url(path));
    _headers.forEach((k, v) => req.headers[k] = v);
    if (body != null) req.body = jsonEncode(body);
    final res = await http.Response.fromStream(await _client.send(req).timeout(const Duration(seconds: 20)));
    dynamic parsed;
    try { parsed = res.body.isEmpty ? null : jsonDecode(res.body); } catch (_) { parsed = res.body; }
    return (res.statusCode, parsed);
  }

  void dispose() => _client.close();

  static http.Client _buildClient(String expectedFp) {
    final normalizedFp = expectedFp.trim().toLowerCase();
    if (normalizedFp.isEmpty) {
      throw StateError('TLS certificate fingerprint is required');
    }
    final ctx = SecurityContext(withTrustedRoots: false);
    final ioClient = HttpClient(context: ctx)
      ..badCertificateCallback = (cert, host, port) {
          final fp = crypto.sha256.convert(cert.der).toString();
          return fp.toLowerCase() == normalizedFp;
        };
    return IOClient(ioClient);
  }
}
