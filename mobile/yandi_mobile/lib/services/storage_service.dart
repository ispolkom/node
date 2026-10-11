import 'dart:convert';
import 'dart:io';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:path/path.dart' as p;
import 'package:path_provider/path_provider.dart';
import 'package:sqflite_sqlcipher/sqflite.dart';
import '../models/trusted_node.dart';
import '../models/message.dart';
import '../crypto/replay_ledger.dart';

/// Единое хранилище приложения.
///
/// flutter_secure_storage  — секреты (ключи идентичности, токены)
/// SQLCipher               — ноды, история сообщений, группы, кэш публичных ключей: вся база зашифрована ключом данных (см. VaultKeys),
///                           который есть только пока приложение открыто паролем
class StorageService {
  static Database? _db;
  static ReplayLedger? replayLedger;

  static const _secure = FlutterSecureStorage(
    aOptions: AndroidOptions(encryptedSharedPreferences: true),
  );

  // ── Инициализация ──────────────────────────────────────────────────────────

  /// Без базы: только то, что не требует ключа (журнал повторов). Базу открывает [openEncrypted] после ввода пароля.
  static Future<void> init() async {
    if (replayLedger != null) return;
    final dir  = await getApplicationDocumentsDirectory();
    replayLedger = ReplayLedger(File(p.join(dir.path,'e2e_replay_e4.json')));
  }

  /// «Удалить всё»: база (все её файлы), журнал повторов, скачанные файлы, аватарки и всё защищённое хранилище (ключи, токены).
  /// Каждый файл затирается (там, где есть нативное затирание) и удаляется; после этого приложение как только что установленное.
  static Future<void> eraseAllLocalData() async {
    await closeDb();
    replayLedger = null;
    final docs = await getApplicationDocumentsDirectory();
    final support = await getApplicationSupportDirectory();
    for (final d in [docs, support]) {
      if (!await d.exists()) continue;
      await for (final e in d.list(recursive: true, followLinks: false)) {
        if (e is File) await _wipeAndDelete(e.path);
      }
      await for (final e in d.list(followLinks: false)) {
        if (e is Directory) {
          try { await e.delete(recursive: true); } catch (_) {}
        }
      }
    }
    await _secure.deleteAll();
    _secureCache = null;
  }

  static bool get dbOpen => _db != null;

  /// Затирание поверх байтов перед удалением (на телефоне — нативное, задаётся при старте приложения); без него файл просто удаляется.
  static Future<bool> Function(String path)? wipeHook;

  static const _plainName = 'yandi.db';       // прежняя нешифрованная база (только для переноса)
  static const _encName   = 'yandi_enc.db';   // зашифрованная

  /// Открыть зашифрованную базу ключом данных [key] (32 байта). Прежняя нешифрованная переносится и затирается.
  /// Неверный ключ — исключение (база не открывается, не пересоздаётся и не портится).
  static Future<void> openEncrypted(List<int> key) async {
    if (_db != null) return;
    final dir = await getApplicationDocumentsDirectory();
    final plainPath = p.join(dir.path, _plainName);
    final encPath   = p.join(dir.path, _encName);
    final pass = _rawKey(key);
    if (await _isPlainSqlite(plainPath)) await _migratePlain(plainPath, encPath, pass);
    _db = await openDatabase(encPath, version: 5, onCreate: _createTables, onUpgrade: _onUpgrade, password: pass);
  }

  /// Закрыть базу (блокировка приложения): после этого без пароля прочесть её нечем.
  static Future<void> closeDb() async {
    final d = _db;
    _db = null;
    if (d != null) await d.close();
  }

  /// SQLCipher принимает готовый ключ в виде `x'<64 hex>'`: без лишнего растяжения, ключ уже случайный.
  static String _rawKey(List<int> key) => "x'${key.map((b) => b.toRadixString(16).padLeft(2, '0')).join()}'";

  static Future<bool> _isPlainSqlite(String path) async {
    final f = File(path);
    if (!await f.exists()) return false;
    final raf = await f.open();
    try {
      final head = await raf.read(16);
      return head.length == 16 && String.fromCharCodes(head) == 'SQLite format 3\u0000';
    } finally {
      await raf.close();
    }
  }

  /// Перенос: читаем все таблицы из старой базы, пишем в новую зашифрованную (через временный файл: оборванный перенос ничего не ломает),
  /// проверяем число строк, затем затираем и удаляем старую вместе с её служебными файлами.
  static Future<void> _migratePlain(String plainPath, String encPath, String pass) async {
    final tmp = '$encPath.tmp';
    for (final f in [tmp, '$tmp-journal', '$tmp-wal', '$tmp-shm']) {
      try { await File(f).delete(); } catch (_) {}
    }
    final src = await openDatabase(plainPath, readOnly: true);
    final dst = await openDatabase(tmp, version: 5, onCreate: _createTables, password: pass);
    try {
      for (final t in const ['nodes', 'peer_pubkeys', 'messages', 'manual_contacts', 'group_entries']) {
        List<Map<String, Object?>> rows;
        try { rows = await src.query(t); } catch (_) { continue; }   // таблицы может не быть в очень старой базе
        for (var i = 0; i < rows.length; i += 200) {
          final b = dst.batch();
          for (final r in rows.skip(i).take(200)) {
            b.insert(t, r, conflictAlgorithm: ConflictAlgorithm.replace);
          }
          await b.commit(noResult: true);
        }
        final copied = Sqflite.firstIntValue(await dst.rawQuery('SELECT COUNT(*) FROM $t'));
        if (copied != rows.length) throw StateError('перенос базы: в таблице $t $copied из ${rows.length} строк');
      }
    } finally {
      await src.close();
      await dst.close();
    }
    await File(tmp).rename(encPath);
    for (final f in [plainPath, '$plainPath-journal', '$plainPath-wal', '$plainPath-shm']) {
      await _wipeAndDelete(f);
    }
  }

  static Future<void> _wipeAndDelete(String path) async {
    final f = File(path);
    if (!await f.exists()) return;
    final w = wipeHook;
    if (w != null) {
      try { await w(path); } catch (_) {}
    }
    if (await f.exists()) {
      try { await f.delete(); } catch (_) {}
    }
  }

  static Future<void> _createGroupTables(Database db) async {
    await db.execute('''
      CREATE TABLE IF NOT EXISTS group_entries (
        gid     TEXT NOT NULL,
        seq     INTEGER NOT NULL,
        ts      INTEGER NOT NULL,
        epoch   INTEGER NOT NULL,
        kind    INTEGER NOT NULL,
        sender  TEXT NOT NULL,
        body    TEXT NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (gid, seq)
      )
    ''');
    await db.execute('CREATE INDEX IF NOT EXISTS idx_group_kind ON group_entries(gid, kind, seq)');
  }

  static Future<void> _onUpgrade(Database db, int oldVersion, int newVersion) async {
    if (oldVersion < 5) await _createGroupTables(db);
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
    await _createGroupTables(db);
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

  /// Блокировка: расшифрованные значения защищённого хранилища из памяти убираем (при следующем входе читаются заново).
  static void dropSecureCache() => _secureCache = null;

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
    await _secure.write(key: key, value: value);
    _secureCache?[key] = value;
  }

  /// Все имена ключей защищённого хранилища (для чистки по шаблону, например всех ключей одной группы).
  static Future<List<String>> secureKeys() async {
    final c = _secureCache;
    if (c != null) return c.keys.toList();
    return (await _secure.readAll()).keys.toList();
  }

  static Future<void> secureDelete(String key) async {
    await _secure.delete(key: key);
    _secureCache?.remove(key);
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
