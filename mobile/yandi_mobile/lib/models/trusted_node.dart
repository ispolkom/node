class TrustedNode {
  final String   id;
  final String   name;
  final String   host;
  final int      port;
  final String   fingerprint;
  final String?  token;
  final int      pingMs;
  final double   loadFactor;
  final int      uptimeHours;
  final String   version;
  final DateTime? lastSeen;
  final bool     isPreferred;
  final int      addedAt;

  const TrustedNode({
    required this.id,
    required this.name,
    required this.host,
    required this.port,
    required this.fingerprint,
    this.token,
    this.pingMs      = 999,
    this.loadFactor  = 0.5,
    this.uptimeHours = 0,
    this.version     = '',
    this.lastSeen,
    this.isPreferred = false,
    required this.addedAt,
  });

  String get wsUrl {
    final scheme = fingerprint.isNotEmpty ? 'wss' : 'ws';
    return '$scheme://$host:$port';
  }

  String get httpUrl {
    final scheme = fingerprint.isNotEmpty ? 'https' : 'http';
    return '$scheme://$host:$port';
  }

  factory TrustedNode.fromJson(Map<String, dynamic> json) {
    return TrustedNode(
      id:          json['id']          as String,
      name:        json['name']        as String,
      host:        json['host']        as String,
      port:        json['port']        as int,
      fingerprint: json['fingerprint'] as String? ?? '',
      token:       json['token']       as String?,
      pingMs:      json['ping_ms']     as int? ?? 999,
      loadFactor:  (json['load_factor'] as num?)?.toDouble() ?? 0.5,
      uptimeHours: json['uptime_hours'] as int? ?? 0,
      version:     json['version']     as String? ?? '',
      lastSeen:    json['last_seen'] != null
          ? DateTime.fromMillisecondsSinceEpoch(json['last_seen'] as int)
          : null,
      isPreferred: (json['is_preferred'] as int? ?? 0) == 1,
      addedAt:     json['added_at']    as int,
    );
  }

  Map<String, dynamic> toJson() => {
    'id':           id,
    'name':         name,
    'host':         host,
    'port':         port,
    'fingerprint':  fingerprint,
    'token':        token,
    'ping_ms':      pingMs,
    'load_factor':  loadFactor,
    'uptime_hours': uptimeHours,
    'version':      version,
    'last_seen':    lastSeen?.millisecondsSinceEpoch,
    'is_preferred': isPreferred ? 1 : 0,
    'added_at':     addedAt,
  };

  TrustedNode copyWith({
    String?   name,
    String?   token,
    int?      pingMs,
    double?   loadFactor,
    int?      uptimeHours,
    String?   version,
    DateTime? lastSeen,
    bool?     isPreferred,
  }) {
    return TrustedNode(
      id:          id,
      name:        name        ?? this.name,
      host:        host,
      port:        port,
      fingerprint: fingerprint,
      token:       token       ?? this.token,
      pingMs:      pingMs      ?? this.pingMs,
      loadFactor:  loadFactor  ?? this.loadFactor,
      uptimeHours: uptimeHours ?? this.uptimeHours,
      version:     version     ?? this.version,
      lastSeen:    lastSeen    ?? this.lastSeen,
      isPreferred: isPreferred ?? this.isPreferred,
      addedAt:     addedAt,
    );
  }

  // Rating: lower ping + lower load = higher score
  double get rating => (1000 - pingMs.clamp(0, 999)) / 1000.0 - loadFactor;

  @override
  String toString() => 'TrustedNode($name @ $host:$port)';
}
