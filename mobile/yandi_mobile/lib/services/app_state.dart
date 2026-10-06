import 'dart:async' as async_lib;
import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:audioplayers/audioplayers.dart';
import '../models/trusted_node.dart';
import '../models/contact.dart';
import '../models/message.dart';
import '../crypto/identity.dart';
import '../crypto/e2e_crypto.dart';
import 'storage_service.dart';
import 'api_service.dart' as api_svc;
import 'ws_service.dart';
import 'vpn_service.dart' show YandiVpnService;
import 'chat_bg_service.dart';
import 'node_manager.dart';
import 'node_discovery.dart';
import 'notification_service.dart';

/// Центральный state приложения.
///
/// Порядок инициализации:
///   1. StorageService.init() — SQLite
///   2. Identity.loadOrGenerate() — ключи пользователя
///   3. NodeManager.load() — список нод из БД
///   4. NodeDiscovery.discoverOnStartup() — найти/обновить ноды
///   5. NodeManager.selectBest() — выбрать лучшую ноду
///   6. Запустить WsService + ApiService
class AppState extends ChangeNotifier {
  // ── Публичные поля ─────────────────────────────────────────────────────────
  Identity?    identity;           // криптографическая личность пользователя
  bool         nodeOnline  = false;
  bool         vpnRunning  = false;
  String?      proxyHost;
  int?         proxyPort;

  List<Contact>                  contacts = [];
  final Map<String, List<ChatMessage>> _chats   = {};

  // ── Сервисы ────────────────────────────────────────────────────────────────
  final NodeManager    nodeManager = NodeManager();
  final YandiVpnService vpn        = YandiVpnService();
  final ChatBgService   chatBg     = ChatBgService();

  api_svc.ApiService? _api;
  WsService?  _ws;

  String? activeChatPeerId;
  final _player = AudioPlayer();

  /// Входящие предложения файлов (для отображения в UI)
  final List<FileOfferEvent> pendingFileOffers = [];

  // ── Accessors ──────────────────────────────────────────────────────────────
  TrustedNode? get activeNode  => nodeManager.activeNode;
  bool         get isPaired    => nodeManager.rankedNodes.isNotEmpty;
  String?      get myPeerId    => identity?.peerId;
  api_svc.ApiService   get apiService  => _api!;

  // Геттеры для экранов
  List<TrustedNode> get nodes      => nodeManager.nodes;
  TrustedNode?      get pair       => activeNode;
  String?           get myNodeId   => identity?.peerId;

  List<ChatMessage> messagesFor(String peerId) => _chats[peerId] ?? [];

  // ── Инициализация ──────────────────────────────────────────────────────────

  Future<void> init() async {
    // 1. SQLite
    await StorageService.init();

    // 2. Ключи пользователя (генерируются один раз и навсегда)
    identity = await Identity.loadOrGenerate();

    // 3. Список нод
    await nodeManager.load();

    // 4. Discovery (только если нет кэша или нод)
    await NodeDiscovery.discoverOnStartup(nodeManager);

    // 5. Выбор лучшей ноды и запуск
    final best = await nodeManager.selectBest();
    if (best != null) {
      await _startServices(best);
    }

    notifyListeners();
  }

  Future<void> _startServices(TrustedNode node) async {
    _api?.dispose();
    _ws?.dispose();

    _api = api_svc.ApiService(node);
    _ws  = WsService(
      identity!,
      onPermanentLoss: _onConnectionLost,
      getPeerPub: (peerId) => _api!.getPeerX25519Pub(peerId),
    );
    _ws!.connect(node);
    _ws!.chatStream.listen(_onIncomingChat);
    _ws!.statusStream.listen(_onPeerStatus);
    _ws!.fileOfferStream.listen(_onFileOffer);

    chatBg.start(node);

    unawaited(_loadInitialData());
  }

  Future<void> _loadInitialData() async {
    try {
      final info = await _api!.getInfo();
      nodeOnline = info['online'] as bool? ?? false;

      // Регистрируем свои публичные ключи на ноде (для E2E входящих)
      if (identity != null) {
        await _api!.registerPublicKeys(
          ed25519PubBase64: identity!.ed25519PubBase64,
          x25519PubBase64:  identity!.x25519PubBase64,
        );
      }

      await refreshContacts();
      await refreshProxyInfo();
      await _syncInbox();
      notifyListeners();
    } catch (_) {}
  }

  /// Скачать накопившиеся сообщения из inbox на ноде, расшифровать и сохранить
  /// в локальный SQLite. Нода фильтрует по токену — каждое устройство видит
  /// только неподтверждённые именно им сообщения (мультиустройство).
  Future<void> _syncInbox() async {
    if (_api == null || myPeerId == null || identity == null) return;
    try {
      // since=0 — нода сама знает что уже видело это устройство (per-token ACK)
      final msgs = await _api!.fetchInbox(0);
      if (msgs.isEmpty) return;

      final ackIds = <int>[];

      for (final m in msgs) {
        final id         = (m['id'] as num).toInt();
        final fromPeerId = m['from_peer_id'] as String? ?? '';
        final payloadB64 = m['payload_b64']  as String? ?? '';
        final tsMs       = (m['ts_ms'] as num?)?.toInt() ?? 0;

        if (payloadB64.isEmpty || fromPeerId.isEmpty) continue;

        final payloadBytes = base64.decode(payloadB64);

        String text;
        if (E2ECrypto.isEncrypted(payloadBytes)) {
          final decrypted = await E2ECrypto.decrypt(
            payloadBytes,
            (theirPub) => identity!.ecdh(theirPub),
          );
          text = decrypted ?? '[не удалось расшифровать]';
        } else {
          text = utf8.decode(payloadBytes, allowMalformed: true);
        }

        final msg = ChatMessage(
          id:        'inbox_$id',
          peerId:    fromPeerId,
          outgoing:  false,
          text:      text,
          timestamp: DateTime.fromMillisecondsSinceEpoch(tsMs),
          status:    MessageStatus.delivered,
        );
        // ConflictAlgorithm.ignore в saveMessage гарантирует идемпотентность
        await StorageService.saveMessage(msg);
        ackIds.add(id);
      }

      if (ackIds.isNotEmpty) await _api!.ackInbox(ackIds);
    } catch (_) {}
  }

  // ── Паринг новой ноды ──────────────────────────────────────────────────────

  Future<void> completePairing(TrustedNode node) async {
    await nodeManager.addNode(node);

    // Запускаемся на новой ноде если это первая или лучше текущей
    final current = nodeManager.activeNode;
    if (current == null || node.rating >= current.rating) {
      await _startServices(node);
    }
    notifyListeners();
  }

  Future<void> setPreferredNode(String id)  => nodeManager.setPreferred(id).then((_) => notifyListeners());
  Future<void> removeNode(String id)        => unpairNode(id);
  Future<void> addManualContact(String peerId, String name) async {
    await StorageService.saveManualContact(peerId, name);
    contacts.add(Contact(peerId: peerId, displayName: name.isEmpty ? peerId.substring(0, 12) : name, online: false, isManual: true));
    notifyListeners();
  }
  Future<void> removeManualContact(String peerId) async {
    await StorageService.deleteManualContact(peerId);
    contacts.removeWhere((c) => c.peerId == peerId);
    notifyListeners();
  }

  Future<void> unpairNode(String nodeId) async {
    await nodeManager.removeNode(nodeId);

    // Если удалили активную — переключаемся на следующую
    if (activeNode?.id == nodeId || activeNode == null) {
      final next = await nodeManager.selectBest();
      if (next != null) {
        _api?.switchNode(next);
        _ws?.switchNode(next);
        chatBg.start(next);
      } else {
        _ws?.disconnect();
        chatBg.stop();
        nodeOnline = false;
      }
    }
    notifyListeners();
  }

  // ── Failover ───────────────────────────────────────────────────────────────

  Future<void> _onConnectionLost() async {
    final next = await nodeManager.failover();
    if (next != null) {
      _api?.switchNode(next);
      _ws?.switchNode(next);
      chatBg.start(next);
      unawaited(_syncInbox());
    }
  }

  // ── Данные ────────────────────────────────────────────────────────────────

  Future<void> refreshContacts() async {
    try {
      contacts = await _api!.getContacts();
      notifyListeners();
    } catch (_) {}
  }

  Future<void> loadChatHistory(String peerId) async {
    try {
      // Сначала из локального SQLite (быстро, без сети)
      final local = await StorageService.loadMessages(peerId);
      if (local.isNotEmpty) {
        _chats[peerId] = local;
        notifyListeners();
      }
      // Потом delta с ноды
      if (_api != null && myPeerId != null) {
        final remote = await _api!.getHistory(peerId, myPeerId!);
        for (final msg in remote) {
          await StorageService.saveMessage(msg);
        }
        _chats[peerId] = await StorageService.loadMessages(peerId);
        notifyListeners();
      }
    } catch (_) {}
  }

  Future<void> sendMessage(String peerId, String text) async {
    // WsService шифрует если есть ключ (получает его сам из кэша)
    _ws?.sendMessage(peerId, text);

    final msg = ChatMessage(
      id:        DateTime.now().millisecondsSinceEpoch.toString(),
      peerId:    peerId,
      outgoing:  true,
      text:      text,
      timestamp: DateTime.now(),
      status:    MessageStatus.pending,
    );
    _chats.putIfAbsent(peerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    notifyListeners();
  }

  Future<void> refreshProxyInfo() async {
    try {
      final info = await _api!.getProxyInfo();
      if (info['running'] == true) {
        proxyHost = info['host'] as String?;
        proxyPort = info['port'] as int?;
      } else {
        proxyHost = null;
        proxyPort = null;
      }
      notifyListeners();
    } catch (_) {}
  }

  // ── VPN ───────────────────────────────────────────────────────────────────

  Future<void> toggleVpn() async {
    if (vpnRunning) {
      await vpn.stop();
      vpnRunning = false;
      notifyListeners();
      return;
    }
    if (proxyHost == null) await refreshProxyInfo();
    if (proxyHost == null) return;

    final ok = await vpn.start(
      socksHost: proxyHost!,
      socksPort: proxyPort!,
      socksUser: 'yandi',
      socksPass: 'yandi123',
    );
    vpnRunning = ok;
    notifyListeners();
  }

  // ── Экранное состояние (для адаптивного пинга) ─────────────────────────────

  void onScreenOn()  => _ws?.onScreenStateChanged(true);
  void onScreenOff() => _ws?.onScreenStateChanged(false);

  String _contactName(String peerId) {
    for (final c in contacts) {
      if (c.peerId == peerId) return c.displayName.isNotEmpty ? c.displayName : peerId.substring(0, 12);
    }
    return peerId.length >= 12 ? peerId.substring(0, 12) : peerId;
  }

  // ── WebSocket события ─────────────────────────────────────────────────────

  void _onIncomingChat(IncomingChatEvent e) async {
    final msg = ChatMessage(
      id:        '${e.fromPeerId}_${e.timestamp.millisecondsSinceEpoch}',
      peerId:    e.fromPeerId,
      outgoing:  false,
      text:      e.text,
      timestamp: e.timestamp,
      status:    MessageStatus.delivered,
    );
    _chats.putIfAbsent(e.fromPeerId, () => []).add(msg);
    await StorageService.saveMessage(msg);

    if (activeChatPeerId != e.fromPeerId) {
      _player.play(AssetSource('sounds/icq.mp3'));
      // Уведомление когда чат не открыт
      final name = _contactName(e.fromPeerId);
      unawaited(NotificationService.showMessage(
        fromPeerId:  e.fromPeerId,
        displayName: name,
        text:        e.text,
      ));
    } else {
      // Чат открыт — убираем старые уведомления от этого пира
      unawaited(NotificationService.cancelForPeer(e.fromPeerId));
    }
    notifyListeners();
  }

  void _onPeerStatus(PeerStatusEvent e) {
    for (final c in contacts) {
      if (c.peerId == e.peerId) {
        c.online = e.online;
        break;
      }
    }
    notifyListeners();
  }

  void _onFileOffer(FileOfferEvent e) {
    pendingFileOffers.add(e);
    _player.play(AssetSource('sounds/icq.mp3'));
    final name = _contactName(e.fromPeerId);
    unawaited(NotificationService.showFileOffer(
      fromPeerId:  e.fromPeerId,
      displayName: name,
      fileName:    e.fileName,
    ));
    notifyListeners();
  }

  /// Снять предложение файла из очереди (после принятия или отклонения)
  void dismissFileOffer(String transferId) {
    pendingFileOffers.removeWhere((e) => e.transferId == transferId);
    notifyListeners();
  }

  @override
  void dispose() {
    _ws?.dispose();
    _api?.dispose();
    _player.dispose();
    super.dispose();
  }
}

void unawaited(async_lib.Future<void> f) { async_lib.unawaited(f); }
