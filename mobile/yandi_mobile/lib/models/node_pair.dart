/// QR-код при сопряжении с нодой.
/// Формат JSON: {"host":"...","port":8766,"pairing_code":"123456","tls_fingerprint":"...","tls":true}
class NodePair {
  final String host;
  final int    port;
  final String pairingCode;
  final String tlsFingerprint;
  final bool   tls;

  const NodePair({
    required this.host,
    required this.port,
    required this.pairingCode,
    required this.tlsFingerprint,
    required this.tls,
  });

  factory NodePair.fromJson(Map<String, dynamic> json) {
    return NodePair(
      host:           json['host']           as String,
      port:           json['port']           as int,
      pairingCode:    json['pairing_code']   as String,
      tlsFingerprint: json['tls_fingerprint'] as String? ?? '',
      tls:            json['tls']            as bool? ?? true,
    );
  }

  Map<String, dynamic> toJson() => {
    'host':            host,
    'port':            port,
    'pairing_code':    pairingCode,
    'tls_fingerprint': tlsFingerprint,
    'tls':             tls,
  };
}
