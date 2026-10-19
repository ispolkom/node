import 'dart:convert';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';
import 'guest_node.dart';

/// Закреплённые ключи подписи списка (hex): совпадают с `bootstrap/signer.pub` и `SIGNER_KEYS` узла (тест следит за совпадением).
const bootstrapSigners = <String>['89d4938a0bdbe9f6e302dabfee0f1d1d651bca3168b52d38edfa07497d042df2'];

/// Почему список не принят.
enum BootstrapProblem { parse, format, unknownSigner, badSignature, expired, rollback }

bool _hex(Object? s, int n) => s is String && s.length == n && RegExp(r'^[0-9a-fA-F]+$').hasMatch(s);

/// `адрес:порт` для телефона: IP или имя и порт 1–65535 (как `valid_mobile_addr` на узле).
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
  if (host.contains(':')) return false; // голый IPv6 без скобок с портом неоднозначен
  return host.length <= 253 && RegExp(r'^[A-Za-z0-9][A-Za-z0-9.\-]*$').hasMatch(host);
}

class BootstrapEntry {
  final String id, key;
  final List<String> addr, roles, mobile;
  final String? region;
  const BootstrapEntry(this.id, this.key, this.addr, this.region, this.roles, this.mobile);
}

/// Подписанный список входных узлов (тот же `bootstrap.json`, что у узлов). Подпись — Ed25519 над меткой `yandi-bootstrap-v1\0` и сжатым JSON
/// документа с пустой подписью в порядке полей узла.
class BootstrapDoc {
  final int format, issued, expires, sequence;
  final String network, signer, signature;
  final List<BootstrapEntry> entries;
  const BootstrapDoc(this.format, this.network, this.issued, this.expires, this.sequence, this.entries, this.signer, this.signature);

  /// Разбор и проверка формы. Неверное — null.
  static BootstrapDoc? parse(String text) {
    try {
      if (text.length > 256 * 1024) return null;
      final m = jsonDecode(text);
      if (m is! Map) return null;
      final es = m['entries'];
      if (m['format'] is! int || m['network'] is! String || m['issued'] is! int || m['expires'] is! int || m['sequence'] is! int || es is! List || m['signer'] is! String || m['signature'] is! String) return null;
      if (es.length > 64) return null;
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
      return BootstrapDoc(m['format'] as int, m['network'] as String, m['issued'] as int, m['expires'] as int, m['sequence'] as int, out, m['signer'] as String, m['signature'] as String);
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
      'signer': signer,
      'signature': '',
    };
    return Uint8List.fromList([...utf8.encode('yandi-bootstrap-v1\u0000'), ...utf8.encode(jsonEncode(doc))]);
  }

  /// Полная проверка: формат, закреплённый ключ, подпись, срок, откат (`knownSequence` — уже известный выпуск). null — всё верно.
  Future<BootstrapProblem?> verify({required int nowSecs, int? knownSequence, List<String> signers = bootstrapSigners}) async {
    if (format != 1) return BootstrapProblem.format;
    if (!signers.any((k) => k.toLowerCase() == signer.toLowerCase())) return BootstrapProblem.unknownSigner;
    if (!_hex(signer, 64) || !_hex(signature, 128)) return BootstrapProblem.badSignature;
    List<int> unhex(String h) => [for (var i = 0; i < h.length; i += 2) int.parse(h.substring(i, i + 2), radix: 16)];
    final ok = await Ed25519().verify(signingBytes(), signature: Signature(unhex(signature), publicKey: SimplePublicKey(unhex(signer), type: KeyPairType.ed25519)));
    if (!ok) return BootstrapProblem.badSignature;
    if (expires <= nowSecs) return BootstrapProblem.expired;
    if (knownSequence != null && sequence < knownSequence) return BootstrapProblem.rollback;
    return null;
  }

  /// Узлы, принимающие гостей: роль `guest` и адрес для телефона. Имя — страна и адрес.
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
