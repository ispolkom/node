import 'dart:convert';
import 'dart:math';
import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';
import 'package:sqflite/sqflite.dart';
import '../models/trusted_node.dart';
import '../models/message.dart';

/// Единое хранилище приложения.
///
/// flutter_secure_storage  — секреты (ключи идентичности, токены)
/// SQLite                  — ноды, история сообщений, кэш публичных ключей
class StorageService {
  static Database? _db;

  static const _secure = FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
  );

  // ── Инициализация ──────────────────────────────────────────────────────────

  static Future<void> init() async {
    final dir  = await getApplicationDocumentsDirectory();
    final path = p.join(dir.path, 'yandi.db');
    _db = await openDatabase(
      path,
      version: 4,
      onCreate: _createTables,
      onUpgrade: _onUpgrade,
    );
  }

  static Future<void> _onUpgrade(Database db, int oldVersion, int newVersion) async {
    if (oldVersion < 4) {
      // Ключи собеседников, закэшированные до подписанных связок, ничем не проверены: стираем, их перезапросят с проверкой подписи.
      try { await db.delete('peer_pubkeys'); } catch (_) {}
    }
    if (oldVersion < 2) {
      try { await db.execute('ALTER TABLE nodes ADD COLUMN version TEXT NOT NULL DEFAULT ""'); } catch (_) {}
    }
    if (oldVersion < 3) {
      // Migrate every legacy bearer token before normal reads, rather than
      // waiting for loadNodes() to visit a row. If secure storage fails, the
      // migration fails closed and the plaintext token is not silently kept.
      final rows = await db.query('nodes', columns: ['id', 'token']);
      for (final row in rows) {
        final id = row['id'] as String?;
        final token = row['token'] as String?;
        if (id == null || token == null || token.isEmpty) continue;
        await secureWrite('$_nodeTokKey$id', token);
        await db.update('nodes', {'token': null}, where: 'id = ?', whereArgs: [id]);
      }
    }
  }

  static Future<void> _createTables(Database db, int version) async {
    await db.execute('''
      CREATE TABLE nodes (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL,
        host         TEXT NOT NULL,
        port         INTEGER NOT NULL,
        fingerprint  TEXT NOT NULL DEFAULT '',
        token        TEXT,
        ping_ms      INTEGER NOT NULL DEFAULT 999,
        load_factor  REAL NOT NULL DEFAULT 0.5,
        uptime_hours INTEGER NOT NULL DEFAULT 0,
        version      TEXT NOT NULL DEFAULT '',
        last_seen    INTEGER,
        is_preferred INTEGER NOT NULL DEFAULT 0,
        added_at     INTEGER NOT NULL
      )
    ''');

    await db.execute('''
      CREATE TABLE peer_pubkeys (
        peer_id      TEXT PRIMARY KEY,
        x25519_pub   TEXT NOT NULL,
        cached_at    INTEGER NOT NULL
      )
    ''');

    await db.execute('''
      CREATE TABLE messages (
        id        TEXT PRIMARY KEY,
        peer_id   TEXT NOT NULL,
        outgoing  INTEGER NOT NULL,
        text      TEXT NOT NULL,
        timestamp INTEGER NOT NULL,
        status    TEXT NOT NULL DEFAULT 'pending'
      )
    ''');

    await db.execute(
      'CREATE INDEX idx_msg_peer ON messages(peer_id, timestamp)',
    );

    await db.execute('''
      CREATE TABLE manual_contacts (
        peer_id  TEXT PRIMARY KEY,
        name     TEXT NOT NULL DEFAULT '',
        added_at INTEGER NOT NULL
      )
    ''');
  }

  static Database get db {
    assert(_db != null, 'StorageService.init() не вызван');
    return _db!;
  }

  // ── Secure storage helpers ─────────────────────────────────────────────────

  // flutter_secure_storage на слабых телефонах читает ~1 с за ключ, а при запуске их читается с десяток (запуск тянулся 10+ с).
  // Поэтому при старте всё читается одним readAll(), дальше — из памяти; запись идёт и в память, и в хранилище.
  static Map<String, String>? _secureCache;

  static Future<void> warmSecureCache() async {
    try {
      _secureCache = await _secure.readAll();
    } catch (_) {
      _secureCache = null; // не вышло — читаем по одному, как раньше
    }
  }

  static Future<String?> secureRead(String key) async {
    final c = _secureCache;
    if (c != null) return c[key];
    return _secure.read(key: key);
  }

  static Future<void> secureWrite(String key, String value) async {
    _secureCache?[key] = value;
    await _secure.write(key: key, value: value);
  }

  static Future<void> secureDelete(String key) async {
    _secureCache?.remove(key);
    await _secure.delete(key: key);
  }

  // ── Ноды ──────────────────────────────────────────────────────────────────

  static const String _nodeTokKey = 'node_token_';

  static Future<List<TrustedNode>> loadNodes() async {
    final rows = await db.query('nodes', orderBy: 'added_at ASC');
    final out = <TrustedNode>[];
    for (final r in rows) {
      final node = TrustedNode.fromJson(r);
      var token = await secureRead('$_nodeTokKey${node.id}');
      final legacy = r['token'] as String?;
      if ((token == null || token.isEmpty) && legacy != null && legacy.isNotEmpty) {
        // #8 аудита: токен раньше лежал в открытой SQLite — переносим в secure storage и чистим колонку
        token = legacy;
        await secureWrite('$_nodeTokKey${node.id}', token);
        await db.update('nodes', {'token': null}, where: 'id = ?', whereArgs: [node.id]);
      }
      out.add((token != null && token.isNotEmpty) ? node.copyWith(token: token) : node);
    }
    return out;
  }

  static Future<void> saveNode(TrustedNode node) async {
    final m = node.toJson();
    m['token'] = null;   // #8: bearer-токен не храним в открытой SQLite
    await db.insert('nodes', m, conflictAlgorithm: ConflictAlgorithm.replace);
    if (node.token != null && node.token!.isNotEmpty) {
      await secureWrite('$_nodeTokKey${node.id}', node.token!);
    }
  }

  static Future<void> updateNodeMetrics(TrustedNode node) async {
    await db.update(
      'nodes',
      {
        'ping_ms':      node.pingMs,
        'load_factor':  node.loadFactor,
        'uptime_hours': node.uptimeHours,
        'version':      node.version,
        'last_seen':    node.lastSeen?.millisecondsSinceEpoch,
        'token':        null,   // #8: не в SQLite
        'is_preferred': node.isPreferred ? 1 : 0,
        'name':         node.name,
      },
      where: 'id = ?',
      whereArgs: [node.id],
    );
    if (node.token != null && node.token!.isNotEmpty) {
      await secureWrite('$_nodeTokKey${node.id}', node.token!);
    }
  }

  static Future<void> deleteNode(String id) async {
    await db.delete('nodes', where: 'id = ?', whereArgs: [id]);
    await secureDelete('$_nodeTokKey$id');
  }

  // ── Кэш публичных ключей пиров ─────────────────────────────────────────────

  static Future<String?> getPeerX25519Pub(String peerId) async {
    final rows = await db.query(
      'peer_pubkeys',
      where:     'peer_id = ?',
      whereArgs: [peerId],
    );
    if (rows.isEmpty) return null;
    final cachedAt = rows.first['cached_at'] as int;
    // Инвалидируем кэш через 24 часа
    if (DateTime.now().millisecondsSinceEpoch - cachedAt > 86400000) return null;
    return rows.first['x25519_pub'] as String;
  }

  static Future<void> cachePeerX25519Pub(String peerId, String x25519PubBase64) async {
    await db.insert('peer_pubkeys', {
      'peer_id':   peerId,
      'x25519_pub': x25519PubBase64,
      'cached_at': DateTime.now().millisecondsSinceEpoch,
    }, conflictAlgorithm: ConflictAlgorithm.replace);
  }

  // ── Сообщения ─────────────────────────────────────────────────────────────

  static Future<List<ChatMessage>> loadMessages(String peerId,
      {int limit = 50, String myNodeId = ''}) async {
    final rows = await db.query(
      'messages',
      where:    'peer_id = ?',
      whereArgs: [peerId],
      orderBy:  'timestamp DESC',
      limit:    limit,
    );
    return rows
        .reversed
        .map((r) => ChatMessage(
              id:        r['id']       as String,
              peerId:    r['peer_id']  as String,
              outgoing:  (r['outgoing'] as int) == 1,
              text:      r['text']     as String,
              timestamp: DateTime.fromMillisecondsSinceEpoch(r['timestamp'] as int),
              status:    _parseStatus(r['status'] as String),
            ))
        .toList();
  }

  static Future<void> saveMessage(ChatMessage msg) async {
    await db.insert(
      'messages',
      {
        'id':        msg.id,
        'peer_id':   msg.peerId,
        'outgoing':  msg.outgoing ? 1 : 0,
        'text':      msg.text,
        'timestamp': msg.timestamp.millisecondsSinceEpoch,
        'status':    msg.status.name,
      },
      conflictAlgorithm: ConflictAlgorithm.ignore,
    );
  }

  static Future<void> updateMessageStatus(String id, MessageStatus status) async {
    await db.update(
      'messages',
      {'status': status.name},
      where: 'id = ?', whereArgs: [id],
    );
  }

  static Future<void> updateMessageText(String id, String text) async {
    await db.update('messages', {'text': text}, where: 'id = ?', whereArgs: [id]);
  }

  static Future<Set<String>> loadEditedIds() async {
    final s = await secureRead('edited_ids');
    if (s == null || s.isEmpty) return {};
    try { return (jsonDecode(s) as List).map((e) => e.toString()).toSet(); } catch (_) { return {}; }
  }

  static Future<void> saveEditedIds(Set<String> ids) =>
      secureWrite('edited_ids', jsonEncode(ids.toList()));

  /// Удалить сообщения по id (только локально у этого пользователя).
  static Future<void> deleteMessages(List<String> ids) async {
    if (ids.isEmpty) return;
    final q = List.filled(ids.length, '?').join(',');
    await db.delete('messages', where: 'id IN ($q)', whereArgs: ids);
  }

  /// Очистить всю переписку с собеседником (локально).
  static Future<void> clearChat(String peerId) async {
    await db.delete('messages', where: 'peer_id = ?', whereArgs: [peerId]);
  }

  // ── Inbox sync timestamp ───────────────────────────────────────────────────

  static Future<int> getLastInboxSyncMs() async {
    final s = await secureRead('last_inbox_sync_ms');
    return int.tryParse(s ?? '') ?? 0;
  }

  static Future<void> setLastInboxSyncMs(int ms) =>
      secureWrite('last_inbox_sync_ms', ms.toString());

  // ── Счётчик непрочитанных по собеседникам (переживает перезапуск) ───────────

  static Future<Map<String, int>> loadUnread() async {
    final s = await secureRead('unread_counts');
    if (s == null || s.isEmpty) return {};
    try {
      final m = jsonDecode(s) as Map<String, dynamic>;
      return m.map((k, v) => MapEntry(k, (v as num).toInt()));
    } catch (_) {
      return {};
    }
  }

  static Future<void> saveUnread(Map<String, int> m) =>
      secureWrite('unread_counts', jsonEncode(m));

  // ── Чёрный список (peerId -> отображаемое имя) ──────────────────────────────

  static Future<Map<String, String>> loadBlacklist() async {
    final s = await secureRead('blacklist');
    if (s == null || s.isEmpty) return {};
    try {
      final m = jsonDecode(s) as Map<String, dynamic>;
      return m.map((k, v) => MapEntry(k, v.toString()));
    } catch (_) {
      return {};
    }
  }

  static Future<void> saveBlacklist(Map<String, String> m) =>
      secureWrite('blacklist', jsonEncode(m));

  // ── Ручные контакты ───────────────────────────────────────────────────────

  static Future<List<Map<String, dynamic>>> loadManualContacts() async {
    final rows = await db.query('manual_contacts', orderBy: 'added_at ASC');
    return rows.cast<Map<String, dynamic>>();
  }

  static Future<void> saveManualContact(String peerId, String name) async {
    await db.insert('manual_contacts', {
      'peer_id':  peerId,
      'name':     name,
      'added_at': DateTime.now().millisecondsSinceEpoch,
    }, conflictAlgorithm: ConflictAlgorithm.replace);
  }

  static Future<void> deleteManualContact(String peerId) async {
    await db.delete('manual_contacts', where: 'peer_id = ?', whereArgs: [peerId]);
  }

  // ── PIN-блокировка ────────────────────────────────────────────────────────

  static Future<bool> hasPinSet() async => (await secureRead('pin_hash')) != null;

  /// Сохранить PIN: sha256(pin || random_salt), соль хранится рядом.
  /// Ключи уже защищены Android Keystore — одна итерация sha256 достаточна.
  static Future<void> savePin(String pin) async {
    final rng   = Random.secure();
    final salt  = List<int>.generate(16, (_) => rng.nextInt(256));
    final saltHex = salt.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
    final hash  = _pinHash(pin, saltHex);
    await secureWrite('pin_salt', saltHex);
    await secureWrite('pin_hash', hash);
  }

  static Future<bool> verifyPin(String pin) async {
    final salt = await secureRead('pin_salt');
    final hash = await secureRead('pin_hash');
    if (salt == null || hash == null) return false;
    return _pinHash(pin, salt) == hash;
  }

  static Future<void> clearPin() async {
    await secureDelete('pin_hash');
    await secureDelete('pin_salt');
  }

  static String _pinHash(String pin, String saltHex) {
    final bytes = utf8.encode(pin + saltHex);
    return crypto.sha256.convert(bytes).toString();
  }

  // ── Обратная совместимость: my_node_id ────────────────────────────────────

  static Future<String?> loadMyNodeId() => secureRead('my_node_id');
  static Future<void>    saveMyNodeId(String id) => secureWrite('my_node_id', id);

  static MessageStatus _parseStatus(String s) => switch (s) {
    'delivered' => MessageStatus.delivered,
    'read'      => MessageStatus.read,
    'failed'    => MessageStatus.failed,
    _           => MessageStatus.pending,
  };
}
