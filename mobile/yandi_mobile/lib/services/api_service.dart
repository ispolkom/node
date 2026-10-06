import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart' as crypto;
import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import '../models/trusted_node.dart';
import '../models/contact.dart';
import '../models/message.dart';
import 'storage_service.dart';

class ApiService {
  TrustedNode _node;
  late http.Client _client;

  ApiService(this._node) {
    _client = _buildClient(_node.fingerprint);
  }

  void switchNode(TrustedNode node) {
    _client.close();
    _node   = node;
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
    return jsonDecode(res.body) as Map<String, dynamic>;
  }

  Future<List<Contact>> getContacts() async {
    final res = await _client
        .get(_url('/mobile/contacts'), headers: _headers)
        .timeout(const Duration(seconds: 10));
    final data = jsonDecode(res.body) as Map<String, dynamic>;
    final list = data['contacts'] as List<dynamic>? ?? [];
    return list.map((e) {
      final m = e as Map<String, dynamic>;
      return Contact(
        peerId:      m['peer_id']      as String,
        displayName: m['display_name'] as String? ?? '',
        online:      m['online']       as bool?   ?? false,
        isManual:    m['is_manual']    as bool?   ?? false,
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
    final cached = await StorageService.getPeerX25519Pub(peerId);
    if (cached != null) return base64.decode(cached);
    try {
      final res = await _client
          .get(_url('/mobile/pubkey/$peerId'), headers: _headers)
          .timeout(const Duration(seconds: 8));
      if (res.statusCode != 200) return null;
      final pubB64 = (jsonDecode(res.body) as Map<String, dynamic>)['x25519_pub'] as String?;
      if (pubB64 == null) return null;
      await StorageService.cachePeerX25519Pub(peerId, pubB64);
      return base64.decode(pubB64);
    } catch (_) {
      return null;
    }
  }

  Future<void> registerPublicKeys({
    required String ed25519PubBase64,
    required String x25519PubBase64,
  }) async {
    try {
      await _client.post(
        _url('/mobile/pubkeys'),
        headers: _headers,
        body: jsonEncode({'ed25519_pub': ed25519PubBase64, 'x25519_pub': x25519PubBase64}),
      ).timeout(const Duration(seconds: 10));
    } catch (_) {}
  }

  Future<List<Map<String, dynamic>>> fetchInbox(int sinceMs) async {
    try {
      final res = await _client
          .get(_url('/mobile/inbox?since=$sinceMs&limit=200'), headers: _headers)
          .timeout(const Duration(seconds: 15));
      if (res.statusCode != 200) return [];
      final data = jsonDecode(res.body) as Map<String, dynamic>;
      return (data['messages'] as List<dynamic>?)?.cast<Map<String, dynamic>>() ?? [];
    } catch (_) {
      return [];
    }
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

  void dispose() => _client.close();

  static http.Client _buildClient(String expectedFp) {
    if (expectedFp.isEmpty) return http.Client();
    final ctx = SecurityContext(withTrustedRoots: false);
    final ioClient = HttpClient(context: ctx)
      ..badCertificateCallback = (cert, host, port) {
          final fp = crypto.sha256.convert(cert.der).toString();
          return fp.toLowerCase() == expectedFp.toLowerCase();
        };
    return IOClient(ioClient);
  }
}
