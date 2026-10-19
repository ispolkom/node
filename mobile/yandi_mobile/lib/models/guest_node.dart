import 'dart:convert';

/// Узел, принимающий гостей: имя, адрес, порт и отпечаток сертификата (соединение только с этим сертификатом).
class GuestNode {
  final String name, host, fingerprint;
  final int port;
  const GuestNode(this.name, this.host, this.port, this.fingerprint);

  /// Разбор списка из вшитого файла или подписанного ответа: только верные записи (адрес, порт 1–65535, отпечаток 64 hex), не больше 16.
  static List<GuestNode> parseList(Object? raw) {
    Object? list = raw;
    if (raw is String) {
      try {
        list = (jsonDecode(raw) as Map)['nodes'];
      } catch (_) {
        return const [];
      }
    }
    if (list is! List) return const [];
    final out = <GuestNode>[];
    for (final x in list) {
      if (x is! Map) continue;
      final name = '${x['name'] ?? ''}'.trim(), host = '${x['host'] ?? ''}'.trim(), fp = '${x['fingerprint'] ?? ''}'.trim().toLowerCase();
      final port = x['port'];
      if (name.isEmpty || name.length > 40 || name.runes.any((r) => r < 32)) continue;
      if (!RegExp(r'^[A-Za-z0-9.\-]{1,253}$|^\[[0-9A-Fa-f:]{2,45}\]$|^[0-9A-Fa-f:]{2,45}$').hasMatch(host)) continue;
      if (port is! int || port < 1 || port > 65535) continue;
      if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(fp)) continue;
      out.add(GuestNode(name, host, port, fp));
      if (out.length >= 16) break;
    }
    return out;
  }
}
