import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'package:flutter/services.dart' show rootBundle;

// Список входных узлов для гостя: копия `mobile/yandi_mobile/lib/models/bootstrap_list.dart` (формат и подпись побайтно те же)
// и `services/bootstrap_loader.dart` без кэша на диске (гость живёт в памяти).

/// Закреплённые ключи подписи списка (hex): совпадают с `bootstrap/signer.pub` и телефоном.
const bootstrapSigners = <String>['52ba508fca1ca54bfe010dbb3d511e51ea4ee06bd59ce0816ae240745fd4025a'];

bool _hex(Object? s, int n) => s is String && s.length == n && RegExp(r'^[0-9a-fA-F]+$').hasMatch(s);

List<int> unhex(String h) => [for (var i = 0; i < h.length; i += 2) int.parse(h.substring(i, i + 2), radix: 16)];

String toHex(List<int> b) => b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();

/// `адрес:порт` для клиента: IP или имя и порт 1–65535 (как `valid_mobile_addr` на узле).
bool validMobileAddr(String a) {
  if (a.length > 262) return false;
  final i = a.lastIndexOf(':');
  if (i <= 0) return false;
  var host = a.substring(0, i);
  final port = int.tryParse(a.substring(i + 1));
  if (port == null || port < 1 || port > 65535) return false;
  if (host.startsWith('[') && host.endsWith(']')) {
    host = host.substring(1, host.length - 1);
    return RegExp(r'^[0-9A-Fa-f:.]{2,45}$').hasMatch(host);
  }
  if (host.contains(':')) return false;
  return host.length <= 253 && RegExp(r'^[A-Za-z0-9][A-Za-z0-9.\-]*$').hasMatch(host);
}

/// Узел, принимающий гостей. [fingerprint] пуст, пока узел не представился (`/mobile/hello`); [id] и [key] — из подписанного списка.
class GuestNode {
  final String name, host, fingerprint, id, key;
  final int port;
  const GuestNode(this.name, this.host, this.port, this.fingerprint, {this.id = '', this.key = ''});
  GuestNode withFingerprint(String fp) => GuestNode(name, host, port, fp, id: id, key: key);
}

class BootstrapEntry {
  final String id, key;
  final List<String> addr, roles, mobile;
  final String? region;
  const BootstrapEntry(this.id, this.key, this.addr, this.region, this.roles, this.mobile);
}

enum BootstrapProblem { format, unknownSigner, badSignature, expired, rollback }

class BootstrapDoc {
  final int format, issued, expires, sequence;
  final String network, signer, signature;
  final List<BootstrapEntry> entries;
  /// Отзыв сборок: версии узла и приложения не ниже этих (в подписи сразу после `entries`, пишутся только заданные).
  final String? minNode, minApp;
  const BootstrapDoc(this.format, this.network, this.issued, this.expires, this.sequence, this.entries, this.signer, this.signature, {this.minNode, this.minApp});

  static BootstrapDoc? parse(String text) {
    try {
      if (text.length > 256 * 1024) return null;
      final m = jsonDecode(text);
      if (m is! Map) return null;
      final es = m['entries'];
      if (m['format'] is! int || m['network'] is! String || m['issued'] is! int || m['expires'] is! int || m['sequence'] is! int || es is! List || m['signer'] is! String || m['signature'] is! String) return null;
      if (es.length > 64) return null;
      if ((m['min_node'] != null && m['min_node'] is! String) || (m['min_app'] != null && m['min_app'] is! String)) return null;
      final out = <BootstrapEntry>[];
      for (final e in es) {
        if (e is! Map || !_hex(e['id'], 64) || !_hex(e['key'], 64)) return null;
        final addr = (e['addr'] as List?)?.cast<String>() ?? const <String>[];
        final roles = (e['roles'] as List?)?.cast<String>() ?? const <String>[];
        final mobile = (e['mobile'] as List?)?.cast<String>() ?? const <String>[];
        final region = e['region'];
        if (region != null && region is! String) return null;
        if (addr.isEmpty || addr.length > 4 || roles.length > 8 || mobile.length > 2) return null;
        if (roles.any((r) => !const ['entry', 'relay', 'exit', 'guest'].contains(r))) return null;
        if (mobile.any((a) => !validMobileAddr(a))) return null;
        if (roles.contains('guest') && mobile.isEmpty) return null;
        out.add(BootstrapEntry((e['id'] as String).toLowerCase(), (e['key'] as String).toLowerCase(), addr, region as String?, roles, mobile));
      }
      return BootstrapDoc(m['format'] as int, m['network'] as String, m['issued'] as int, m['expires'] as int, m['sequence'] as int, out, m['signer'] as String, m['signature'] as String, minNode: m['min_node'] as String?, minApp: m['min_app'] as String?);
    } catch (_) {
      return null;
    }
  }

  /// Что подписано: метка и сжатый JSON в порядке полей узла (`region` пишется всегда, `mobile` — только непустой).
  Uint8List signingBytes() {
    final doc = <String, Object?>{
      'format': format,
      'network': network,
      'issued': issued,
      'expires': expires,
      'sequence': sequence,
      'entries': [
        for (final e in entries)
          <String, Object?>{'id': e.id, 'key': e.key, 'addr': e.addr, 'region': e.region, 'roles': e.roles, if (e.mobile.isNotEmpty) 'mobile': e.mobile},
      ],
      if (minNode != null) 'min_node': minNode,
      if (minApp != null) 'min_app': minApp,
      'signer': signer,
      'signature': '',
    };
    return Uint8List.fromList([...utf8.encode('yandi-bootstrap-v1\u0000'), ...utf8.encode(jsonEncode(doc))]);
  }

  Future<BootstrapProblem?> verify({required int nowSecs, int? knownSequence, List<String> signers = bootstrapSigners}) async {
    if (format != 1) return BootstrapProblem.format;
    if (!signers.any((k) => k.toLowerCase() == signer.toLowerCase())) return BootstrapProblem.unknownSigner;
    if (!_hex(signer, 64) || !_hex(signature, 128)) return BootstrapProblem.badSignature;
    final ok = await Ed25519().verify(signingBytes(), signature: Signature(unhex(signature), publicKey: SimplePublicKey(unhex(signer), type: KeyPairType.ed25519)));
    if (!ok) return BootstrapProblem.badSignature;
    if (expires <= nowSecs) return BootstrapProblem.expired;
    if (knownSequence != null && sequence < knownSequence) return BootstrapProblem.rollback;
    return null;
  }

  /// Узлы, принимающие гостей: роль `guest` и адрес для клиента.
  List<GuestNode> guestNodes() {
    final out = <GuestNode>[];
    for (final e in entries) {
      if (!e.roles.contains('guest')) continue;
      for (final a in e.mobile) {
        final i = a.lastIndexOf(':');
        var host = a.substring(0, i);
        if (host.startsWith('[') && host.endsWith(']')) host = host.substring(1, host.length - 1);
        final label = '${e.region == null ? '' : '${e.region} · '}$host';
        out.add(GuestNode(label.length > 40 ? label.substring(0, 40) : label, host, int.parse(a.substring(i + 1)), '', id: e.id, key: e.key));
        if (out.length >= 16) return out;
      }
    }
    return out;
  }
}

/// Откуда взят список.
enum BootstrapSource { network, embedded }

class BootstrapResult {
  final BootstrapDoc doc;
  final BootstrapSource source;
  const BootstrapResult(this.doc, this.source);
}

/// Сначала сайт, затем вшитая копия; каждый вариант проходит ту же проверку (ключ, подпись, срок, откат не ниже вшитой копии).
/// Все источники подставляются, чтобы проверять без сети.
class GuestBootstrapLoader {
  static const url = 'https://yandi.su/update/bootstrap.json';
  final Future<String?> Function(String url) fetch;
  final Future<String> Function() embedded;
  final int Function() nowSecs;
  final List<String> signers;

  GuestBootstrapLoader({Future<String?> Function(String url)? fetch, Future<String> Function()? embedded, int Function()? nowSecs, this.signers = bootstrapSigners})
      : fetch = fetch ?? _httpFetch,
        embedded = embedded ?? (() => rootBundle.loadString('assets/bootstrap.json', cache: false)),
        nowSecs = nowSecs ?? (() => DateTime.now().millisecondsSinceEpoch ~/ 1000);

  /// Только https и обычные корни системы; ответ ограничен по размеру и времени.
  static Future<String?> _httpFetch(String url) async {
    if (!url.startsWith('https://')) return null;
    final c = HttpClient()..connectionTimeout = const Duration(seconds: 8);
    try {
      final req = await c.getUrl(Uri.parse(url)).timeout(const Duration(seconds: 8));
      final res = await req.close().timeout(const Duration(seconds: 8));
      if (res.statusCode != 200) return null;
      final bytes = <int>[];
      await for (final chunk in res.timeout(const Duration(seconds: 8))) {
        bytes.addAll(chunk);
        if (bytes.length > 256 * 1024) return null;
      }
      return utf8.decode(bytes);
    } catch (_) {
      return null;
    } finally {
      c.close(force: true);
    }
  }

  Future<BootstrapDoc?> _good(String? text, {int? known}) async {
    if (text == null) return null;
    final d = BootstrapDoc.parse(text);
    if (d == null) return null;
    return await d.verify(nowSecs: nowSecs(), knownSequence: known, signers: signers) == null ? d : null;
  }

  /// Лучший проверенный список; null — ни один источник не дал верного.
  Future<BootstrapResult?> load() async {
    final emb = await _good(await embedded().catchError((_) => ''));
    final net = await _good(await fetch(url), known: emb?.sequence);
    if (net != null) return BootstrapResult(net, BootstrapSource.network);
    return emb == null ? null : BootstrapResult(emb, BootstrapSource.embedded);
  }
}
