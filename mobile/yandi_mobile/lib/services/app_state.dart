import 'group_service.dart';
import 'group_api_http.dart';
import 'group_store_sqlite.dart';
import 'files_host.dart';
import 'dart:async' as async_lib;
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;
import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart' as pp;
import 'package:flutter/services.dart' show MethodChannel;
import 'package:audioplayers/audioplayers.dart';
import '../models/trusted_node.dart';
import '../models/contact.dart';
import '../models/message.dart';
import '../models/file_offer.dart';
import '../models/file_state.dart';
import '../models/receipt.dart';
import '../models/avatar_msg.dart';
import 'avatar_service.dart';
import 'avatar_processor.dart';
import '../crypto/identity.dart';
import 'storage_service.dart';
import '../crypto/file_vault.dart';
import 'vault_bytes_cache.dart';
import 'vault_files.dart';
import 'vault_service.dart';
import 'api_service.dart' as api_svc;
import 'ws_service.dart';
import 'vpn_service.dart' show YandiVpnService;
import 'chat_bg_service.dart';
import 'node_manager.dart';
import 'node_discovery.dart';
import 'notification_service.dart';
import 'file_transfer_service.dart';
import 'call_service.dart';
import '../models/call_signal.dart';
import '../screens/call_screen.dart';
import 'package:flutter/material.dart' show MaterialPageRoute;

/// Центральный state приложения.
///
/// Порядок инициализации:
///   1. StorageService.init() — SQLite
///   2. Identity.loadOrGenerate() — ключи пользователя
///   3. NodeManager.load() — список нод из БД
///   4. NodeDiscovery.discoverOnStartup() — найти/обновить ноды
///   5. NodeManager.selectBest() — выбрать лучшую ноду
///   6. Запустить WsService + ApiService
class AppState extends ChangeNotifier implements FilesHost {
  // ── Публичные поля ─────────────────────────────────────────────────────────
  Identity?    identity;           // криптографическая личность пользователя
  bool         nodeOnline  = false;
  /// Почему не подключились к узлу (показать пользователю); null — всё в порядке.
  String?      nodeError;
  /// Своя клавиатура внутри приложения (системная видит каждое нажатие). По умолчанию включена; можно выключить в настройках
  /// (например, нужен голосовой ввод или экранный диктор).
  bool         secureKeyboard = true;
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
    await StorageService.warmSecureCache();
    _unread.addAll(await StorageService.loadUnread());
    _blocked.addAll(await StorageService.loadBlacklist());
    _editedIds.addAll(await StorageService.loadEditedIds());
    secureKeyboard = (await StorageService.secureRead('pref_secure_keyboard')) != '0';

    // аватарки: изолированная папка приложения + соответствие «контакт → хэш» в защищённом хранилище
    final docs = await pp.getApplicationDocumentsDirectory();
    final av = AvatarService(
      dir: Directory('${docs.path}/avatars'),
      read: () => StorageService.secureRead('avatars_v1'),
      write: (s) => StorageService.secureWrite('avatars_v1', s),
      wipe: AppState.wipeFile,
      sealer: AvatarSealer(() => VaultService.instance.subKey('avatars')),
    );
    await av.load();
    av.addListener(notifyListeners);
    avatars = av;

    unawaited(_filesHousekeeping());
    unawaited(StorageService.replayLedger?.reseal().catchError((_) {}) ?? Future<void>.value());   // старый открытый журнал повторов — в зашифрованный вид

    // 2. Ключи пользователя (генерируются один раз и навсегда)
    identity = await Identity.loadOrGenerate();

    // 3. Список нод
    await nodeManager.load();

    // 4. Узлов нет — пробуем bootstrap. Есть — подключаемся сразу к выбранному по сохранённым метрикам, а опрос всех узлов идёт
    // в фоне: раньше он шёл дважды подряд до первого экрана (около 2 с, а с недоступным узлом — до 20 с).
    if (nodeManager.nodes.isEmpty) await NodeDiscovery.discoverOnStartup(nodeManager);
    final best = nodeManager.activeNode;
    unawaited(nodeManager.pingAll());
    if (best != null) {
      await _startServices(best);
    }

    notifyListeners();
  }

  Future<void> _startServices(TrustedNode node) async {
    _api?.dispose();
    _ws?.dispose();

    try {
      _api = api_svc.ApiService(node, onSecurityError: (error) {
        nodeError = error;
        notifyListeners();
      });
    } on StateError catch (e) {
      // узел без отпечатка TLS (запись старой сборки): без закрепления сертификата не подключаемся
      _api = null;
      _ws = null;
      nodeOnline = false;
      nodeError = 'Узел ${node.name}: нет отпечатка сертификата. Сопрягите телефон с узлом заново (QR). ($e)';
      notifyListeners();
      return;
    }
    nodeError = null;
    groups = GroupService(HttpGroupApi(_api!), SqliteGroupStore(), onChanged: (_) => notifyListeners());
    _ws  = WsService(
      identity!,
      onPermanentLoss: _onConnectionLost,
      onOpen: () {
        // сокет открылся (первый раз или после обрыва): статусы за время обрыва не приходили, берём заново, и забираем накопленную почту
        unawaited(refreshContacts());
        unawaited(_syncInbox());
        announceReceiptSupport();
        unawaited(syncGroups());
      },
      getPeerBundle: (peerId) => _api!.getPeerBundle(peerId),
      ownPeerId: () => _api!.devicePeerId(),
    );
    _ws!.connect(node);
    _ws!.chatStream.listen(_onIncomingChat);
    _ws!.statusStream.listen(_onPeerStatus);
    _ws!.fileOfferStream.listen(_onFileOffer);
    _ws!.liveStream.listen(_onLiveSignal);
    _ws!.offlineStream.listen(calls.onPeerOffline);
    _ws!.groupStream.listen(_onGroupHint);

    chatBg.start(node);

    // статусы «в сети» ещё и подтягиваем раз в 20 секунд: события о смене могут потеряться при обрыве связи
    _presenceTimer?.cancel();
    _presenceTimer = async_lib.Timer.periodic(const Duration(seconds: 20), (_) => unawaited(refreshContacts()));

    unawaited(_loadInitialData());
  }

  async_lib.Timer? _presenceTimer;

  /// Группы узла (ключи, журнал, правила). Пересоздаётся вместе с подключением к узлу.
  GroupService? groups;

  /// Для экранов: перерисоваться после действия над группами.
  void notifyListenersPublic() => notifyListeners();

  Future<void> syncGroups({String? only}) async {
    final g = groups;
    if (g == null) return;
    try {
      await g.syncAndAnnounce();
    } catch (_) {}
    notifyListeners();
  }

  void _onGroupHint(GroupHintEvent h) {
    final g = groups;
    if (g == null) return;
    if (h.what == 1) {
      // новая запись — обычная синхронизация; изменённая (стёртая) старая — перечитать именно её
      unawaited(() async {
        final last = await SqliteGroupStore().lastSeq(h.gid);
        if (h.seq > 0 && h.seq <= last) {
          try { await g.refreshEntry(h.gid, h.seq); } catch (_) {}
        } else {
          await syncGroups();
        }
      }());
    } else {
      unawaited(syncGroups());
    }
  }

  Future<void> _loadInitialData() async {
    unawaited(_initDownloadsDir());
    try {
      final info = await _api!.getInfo();
      nodeOnline = info['online'] as bool? ?? false;
      _nodePeerId = info['node_id'] as String?;   // сам компьютер — не «человек»: аватарки ему не шлём

      // Регистрируем свои публичные ключи на ноде (для E2E входящих)
      if (identity != null) {
        await _api!.registerPublicKeys(
          ed25519PubBase64: identity!.ed25519PubBase64,
          x25519PubBase64:  identity!.x25519PubBase64,
          signatureBase64:   await identity!.keyBundleSignatureBase64(),
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
    final ws = _ws;
    if (_api == null || ws == null || myPeerId == null || identity == null) return;
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
        if (payloadBytes.isNotEmpty && (payloadBytes[0] == 0xE2 || payloadBytes[0] == 0xE3)) {
          // Preserve legacy unread mail: never ACK/delete it as an E4 failure.
          nodeError = 'В очереди есть старые неподписанные E2/E3 сообщения. Они сохранены на узле; нужен согласованный перенос.';
          notifyListeners();
          continue;
        }

        // Как и в сокете: только E2E-конверт, общее окно повторов. Открытый текст, старый формат, подделку или повтор (то же
        // сообщение уже пришло вживую) не показываем, но подтверждаем, чтобы узел не присылал его снова.
        var duplicate = false;
        final text = await ws.decryptIncoming(payloadBytes,fromPeerId:fromPeerId,onDuplicate:()=>duplicate=true);
        if (text == null) {
          // Only a verified persisted replay may be ACKed. Missing keys, offline
          // info or disk errors must not silently delete valid queued mail.
          if (duplicate) ackIds.add(id);
          continue;
        }

        if (isBlocked(fromPeerId)) { ackIds.add(id); continue; }   // чёрный список
        final res = _classifyIncoming(fromPeerId, text);
        if (res == null) { ackIds.add(id); continue; } // служебное — подтверждаем и пропускаем
        final showText = res.$1;
        final remoteId = res.$2;
        final msg = ChatMessage(
          id:        'inbox_$id',
          peerId:    fromPeerId,
          outgoing:  false,
          text:      showText,
          timestamp: DateTime.fromMillisecondsSinceEpoch(tsMs),
          status:    MessageStatus.delivered,
          remoteId:  remoteId,
        );
        // ConflictAlgorithm.ignore в saveMessage гарантирует идемпотентность
        await StorageService.saveMessage(msg);
        if (activeChatPeerId != fromPeerId) _bumpUnread(fromPeerId);
        if (remoteId != null) _sendDeliveredReceipt(fromPeerId, remoteId);
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
  Contact? contactFor(String peerId) {
    for (final c in contacts) {
      if (c.peerId == peerId) return c;
    }
    return null;
  }

  bool isSaved(String peerId) => contactFor(peerId)?.saved ?? false;

  String _plainName(String name) => name.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim();

  /// Добавить в контакты: новый человек по Peer ID или уже видимое устройство.
  Future<void> addManualContact(String peerId, String name) async {
    final existing = contactFor(peerId);
    final shown = name.trim().isNotEmpty
        ? name.trim()
        : (existing != null ? _plainName(existing.displayName) : '');
    final finalName = shown.isNotEmpty ? shown : peerId.substring(0, 12);
    await StorageService.saveManualContact(peerId, finalName);
    if (existing != null) {
      contacts[contacts.indexOf(existing)] = Contact(
        peerId: peerId, displayName: finalName, online: existing.online,
        isManual: existing.isManual, saved: true, lastActivityMs: existing.lastActivityMs,
      );
    } else {
      contacts.add(Contact(peerId: peerId, displayName: finalName, online: false, isManual: true));
    }
    notifyListeners();
    _announceAvatar(only: [peerId]);   // добавили в контакты — сообщаем свою картинку
  }

  /// Убрать из контактов. Контакты, заведённые на самом узле, остаются (их удаляют там).
  Future<void> removeManualContact(String peerId) async {
    await StorageService.deleteManualContact(peerId);
    unawaited(avatars?.forgetPeer(peerId) ?? Future<void>.value());   // не в контактах — картинку не показываем и не храним
    _avatarAnnounced.remove(peerId);
    final c = contactFor(peerId);
    if (c != null && !c.isManual) {
      contacts[contacts.indexOf(c)] = c.copyWith(saved: false);
    } else {
      contacts.removeWhere((x) => x.peerId == peerId);
    }
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
      final fromNode = await _api!.getContacts();
      // то, что человек добавил в приложении, не должно пропадать при обновлении списка с узла
      final saved = <String, String>{
        for (final r in await StorageService.loadManualContacts())
          r['peer_id'] as String: (r['name'] as String?) ?? '',
      };
      final merged = <Contact>[];
      final seen = <String>{};
      for (final c in fromNode) {
        seen.add(c.peerId);
        final name = saved[c.peerId];
        merged.add(name == null || name.isEmpty
            ? c
            : Contact(peerId: c.peerId, displayName: c.isManual ? c.displayName : name, online: c.online, isManual: c.isManual, saved: true, lastActivityMs: c.lastActivityMs));
      }
      for (final e in saved.entries) {
        if (seen.contains(e.key)) continue;
        // знакомый только по Peer ID: «в сети», если узел видит его под тем же полным номером
        merged.add(Contact(peerId: e.key, displayName: e.value.isEmpty ? e.key.substring(0, 12) : e.value, online: false, isManual: true));
      }
      contacts = merged.where((c) => !isBlocked(c.peerId)).toList();
      notifyListeners();
      announceReceiptSupport();
      _resyncAvatarsIfDue();
      _announceAvatar();
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

  // ── Звонки ─────────────────────────────────────────────────────────────────

  late final CallService calls = CallService(
    sendSignal: (peerId, text) async => await (_ws?.sendLive(peerId, text) ?? Future.value(false)),
    fetchTurn:  () async => await _api?.getTurn(),
    nodeHost:   () => nodeManager.activeNode?.host,
    nameOf:     (peerId) => contactFor(peerId)?.displayName ?? _contactName(peerId),
    showScreen: _showCallScreen,
    onIncomingNotify: (peerId, name, video) =>
        unawaited(NotificationService.showIncomingCall(fromPeerId: peerId, displayName: name, video: video)),
    onIncomingCancel: () => unawaited(NotificationService.cancelIncomingCall()),
    onLog: (peerId, outgoing, text) => unawaited(addLocalMessage(peerId, outgoing, text)),
  );

  bool _callScreenOpen = false;

  void _showCallScreen() {
    if (_callScreenOpen) return;
    final nav = NotificationService.navigatorKey.currentState;
    if (nav == null) return;
    _callScreenOpen = true;
    nav.push(MaterialPageRoute(builder: (_) => const CallScreen())).whenComplete(() => _callScreenOpen = false);
  }

  /// Позвонить своему устройству. null — звонок начат, иначе причина отказа.
  Future<String?> startCall(String peerId, {required bool video}) async {
    if (!await canSendFiles(peerId)) {
      return 'Звонить можно только на свои устройства (у этого собеседника нет ключа шифрования)';
    }
    return calls.start(peerId, withVideo: video);
  }

  /// Запись в чат, которую видит только этот телефон (итог звонка).
  Future<void> addLocalMessage(String peerId, bool outgoing, String text) async {
    final msg = ChatMessage(
      id: 'local_${DateTime.now().microsecondsSinceEpoch}', peerId: peerId, outgoing: outgoing, text: text,
      timestamp: DateTime.now(), status: MessageStatus.delivered,
    );
    _chats.putIfAbsent(peerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    notifyListeners();
  }

  void _onLiveSignal(LiveSignalEvent e) {
    if (isBlocked(e.fromPeerId)) return;   // чёрный список: звонки/сигналы игнорируем
    if (MsgChannel.isCapPing(e.text)) { _receiptCapable.add(e.fromPeerId); return; }
    final sig = CallSignal.tryParse(e.text);
    if (sig != null) unawaited(calls.onSignal(e.fromPeerId, sig));
  }

  // ── Файлы ──────────────────────────────────────────────────────────────────

  /// Состояние передач по номеру: идёт ли загрузка/скачивание, прогресс, ошибка.
  final Map<String, FileState> fileStates = {};
  /// Откуда отправлен исходящий файл (для превью и открытия), пока приложение запущено.
  final Map<String, String> _outgoingPaths = {};
  Directory? _downloadsDir;

  FileState? fileStateOf(String tid) => fileStates[tid];

  /// Где лежит полученный файл, если он уже скачан.
  String? localPathFor(FileOffer o) {
    final dir = _downloadsDir;
    if (dir == null) return null;
    final p = '${dir.path}/${FileTransferService.localName(o)}';
    return File(p).existsSync() ? p : _outgoingPaths[o.tid];
  }

  Future<void> _initDownloadsDir() async {
    _downloadsDir ??= await FileTransferService.downloadsDir();
  }

  VaultFiles? _vaultFiles;

  /// Открытая копия, которую сделал выбор файла (в кеше приложения), после шифрования затирается; чужие файлы не трогаем.
  Future<void> dropPickerCopy(String path) async {
    try {
      final cache = await pp.getTemporaryDirectory();
      if (!path.startsWith(cache.path)) return;
      if (!await wipeFile(path)) {
        final f = File(path);
        if (await f.exists()) await f.delete();
      }
    } catch (_) {}
  }

  /// Хранилище файлов (зашифрованное) с папкой `yandi_downloads`; временные расшифрованные копии — в кеше приложения.
  Future<VaultFiles> vaultFiles() async {
    final v = _vaultFiles;
    if (v != null) return v;
    final tmp = await pp.getTemporaryDirectory();
    return _vaultFiles = VaultFiles(await FileTransferService.downloadsDir(),
        vault: VaultService.instance.files, wipe: AppState.wipeFile, tempDir: Directory('${tmp.path}/vtmp'));
  }

  /// После входа: остатки временных расшифрованных копий затираются, недоделанные удаления дочищаются, старые открытые файлы
  /// (до включения защиты) шифруются.
  Future<void> _filesHousekeeping() async {
    try {
      final vf = await vaultFiles();
      await vf.cleanTemp();
      await vf.finishPendingWipes();
      await vf.migrateLegacy();
    } catch (_) {}
  }

  /// Файлы можно слать только на своё устройство: у него есть ключ для E2E.
  Future<bool> canSendFiles(String peerId) async => await (_ws?.canEncryptTo(peerId) ?? Future.value(false));

  /// Отправить файл: шифрование на телефоне, куски на узел, ключ получателю в зашифрованном сообщении.
  @override
  int get transferCount => fileStates.length;

  @override
  Future<String?> sendFile(String peerId, String path, {String? name}) async {
    if (_api == null || _ws == null) return 'Нет связи с узлом';
    final svc = FileTransferService(_api!, vault: await VaultService.instance.files());
    late FileOffer offer;
    try {
      if (!await canSendFiles(peerId)) return 'Файлы можно отправлять только на свои устройства, у этого собеседника нет ключа';
      offer = await svc.prepare(toPeerId: peerId, path: path, name: name);
    } on TransferException catch (e) {
      return e.message;
    } catch (e) {
      return 'Не удалось начать отправку: $e';
    }
    // Источник отправки — всегда зашифрованная копия в хранилище под номером передачи: открытая копия из выбора файла не остаётся,
    // а отправленный файл виден во вкладке «Файлы» и в чате.
    var source = path;
    var created = false;
    if (!FileVault.isVaultPath(path)) {
      try {
        final vf = await vaultFiles();
        source = (await vf.importAs(path, offer.tid, name: name, mime: offer.mime)).path;
        created = true;
      } catch (e) {
        try { await _api!.deleteTransfer(offer.tid); } catch (_) {}
        return 'Не удалось положить файл в защищённое хранилище: $e';
      }
      await dropPickerCopy(path);
    }
    _outgoingPaths[offer.tid] = source;
    fileStates[offer.tid] = FileState.working(0);
    // сообщение в чате появляется сразу и показывает ход отправки; получателю предложение уйдёт, когда всё залито
    final msg = ChatMessage(
      id: 'file_${offer.tid}', peerId: peerId, outgoing: true, text: offer.encode(),
      timestamp: DateTime.now(), status: MessageStatus.pending,
    );
    _chats.putIfAbsent(peerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    notifyListeners();
    try {
      final done = await svc.upload(offer, source, onProgress: (f) {
        fileStates[offer.tid] = FileState.working(f);
        notifyListeners();
      });
      final sent = await _ws!.sendEncryptedOnly(peerId, done.encode());
      if (!sent) throw TransferException('Файл залит, но предложение не отправлено: нет связи или ключа получателя');
      fileStates.remove(offer.tid);
      final delivered = msg.copyWith(status: MessageStatus.delivered);
      _replaceMessage(peerId, msg.id, delivered);
      // saveMessage вставляет с ignore и уже существующую строку не обновит: статус меняем явно
      await StorageService.updateMessageStatus(msg.id, MessageStatus.delivered);
    } catch (e) {
      fileStates[offer.tid] = FileState.failed(e is TransferException ? e.message : '$e');
      if (created) {
        try { await (await vaultFiles()).deleteForever(source); } catch (_) {}
        _outgoingPaths.remove(offer.tid);
      }
      final failed = msg.copyWith(status: MessageStatus.failed);
      _replaceMessage(peerId, msg.id, failed);
      await StorageService.updateMessageStatus(msg.id, MessageStatus.failed);
    }
    notifyListeners();
    return null;
  }

  void _replaceMessage(String peerId, String id, ChatMessage m) {
    final list = _chats[peerId];
    if (list == null) return;
    final i = list.indexWhere((x) => x.id == id);
    if (i >= 0) list[i] = m;
  }

  /// Скачать присланный файл.
  Future<void> downloadFile(FileOffer o) async {
    if (_api == null) return;
    await _initDownloadsDir();
    fileStates[o.tid] = FileState.working(0);
    notifyListeners();
    try {
      await FileTransferService(_api!, vault: await VaultService.instance.files()).download(o, onProgress: (f) {
        fileStates[o.tid] = FileState.working(f);
        notifyListeners();
      });
      fileStates.remove(o.tid);
    } catch (e) {
      fileStates[o.tid] = FileState.failed(e is TransferException ? e.message : 'Не удалось скачать: $e');
    }
    notifyListeners();
  }

  static const MethodChannel _filesChannel = MethodChannel('com.yandi.yandi_mobile/files');

  /// Нативное затирание файла поверх его байтов и удаление (см. FileWipe.java); false — не вышло, вызывающий удалит обычным способом.
  static Future<bool> wipeFile(String path) async {
    try {
      return await _filesChannel.invokeMethod<bool>('wipe', {'path': path}) ?? false;
    } catch (_) {
      return false;
    }
  }

  /// Скопировать полученный файл в общую папку «Загрузки», чтобы он был виден в файловом менеджере.
  Future<String?> saveToDownloads(String path, String name, String mime) async {
    try {
      // наружу уходит расшифрованная копия (это и есть «вынести»); временный файл затирается сразу после сохранения
      final vf = await vaultFiles();
      return await vf.withPlain(path, (plain) => _filesChannel.invokeMethod<String>('saveToDownloads', {'path': plain, 'name': name, 'mime': mime}));
    } catch (_) {
      return null;
    }
  }

  // ── Статусы доставки (квитанции, всё E2E, узел их не отличает от чата) ──────
  //
  // Квитанции ходят как обычные зашифрованные сообщения с меткой: «доставлено» шлёт получатель при приёме, «прочитано» — при
  // открытии чата. Чтобы на не обновлённой сборке не показывались служебные строки, конверт сообщения мы отправляем только тем
  // устройствам, что подтвердили поддержку коротким E2E-пингом (старые сборки пинг игнорируют, им идёт обычный текст).

  final Set<String> _receiptCapable = {};   // устройства, умеющие квитанции
  final Set<String> _readAcked = {};         // remoteId, по которым уже отправили «прочитано»
  final Map<String, int> _unread = {};       // непрочитанные по собеседникам (переживают перезапуск)
  final Map<String, String> _blocked = {};    // чёрный список: peerId -> имя (двусторонний, app-side)

  bool isBlocked(String peerId) => _blocked.containsKey(peerId);
  List<MapEntry<String, String>> get blockedContacts => _blocked.entries.toList();

  final Set<String> _editedIds = {};          // id сообщений с меткой «изменено» (persist)
  bool isEdited(String msgId) => _editedIds.contains(msgId);

  ChatMessage _withText(ChatMessage m, String text) => ChatMessage(
    id: m.id, peerId: m.peerId, outgoing: m.outgoing, text: text,
    timestamp: m.timestamp, status: m.status, remoteId: m.remoteId,
  );

  /// Изменить своё отправленное сообщение. Текст меняется и у вас, и у получателя (у него помечается «изменено»).
  /// Удалить у получателя нельзя, а изменить можно — осознанно (см. метку).
  Future<void> editMessage(String peerId, String msgId, String newText) async {
    final t = newText.trim();
    if (t.isEmpty) return;
    final list = _chats[peerId];
    if (list == null) return;
    final i = list.indexWhere((m) => m.id == msgId && m.outgoing);
    if (i < 0) return;
    list[i] = _withText(list[i], t);
    _editedIds.add(msgId);
    await StorageService.updateMessageText(msgId, t);
    await StorageService.saveEditedIds(_editedIds);
    // отправляем правку собеседнику (конверт с тем же id); если не умеет — просто не применит
    unawaited(_ws?.sendMessage(peerId, MsgChannel.editMessage(msgId, t)) ?? Future<bool>.value(false));
    notifyListeners();
  }

  /// Пришла правка от собеседника: находим его сообщение по remoteId и заменяем текст + метка «изменено».
  void _applyEdit(String from, String cmid, String newText) {
    final list = _chats[from];
    if (list == null) return;
    final i = list.indexWhere((m) => !m.outgoing && m.remoteId == cmid);
    if (i < 0) return;
    final local = list[i];
    list[i] = _withText(local, newText);
    _editedIds.add(local.id);
    unawaited(StorageService.updateMessageText(local.id, newText));
    unawaited(StorageService.saveEditedIds(_editedIds));
    notifyListeners();
  }

  Future<void> setSecureKeyboard(bool on) async {
    secureKeyboard = on;
    await StorageService.secureWrite('pref_secure_keyboard', on ? '1' : '0');
    notifyListeners();
  }

  // ── Аватарки ───────────────────────────────────────────────────────────────
  // Картинку получают и показывают только сохранённые контакты (не заблокированные, не сам узел). Раз в сеанс на контакта и раз в 6 часов
  // сообщается только хэш (70 байт); картинка едет, лишь если у собеседника такой ещё нет. Всё — служебные E2E-конверты.

  AvatarService? avatars;
  String? _nodePeerId;
  final Set<String> _avatarAnnounced = {};
  DateTime _lastAvatarSync = DateTime.now();

  Uint8List? avatarBytes(String peerId) => avatars?.bytesForPeer(peerId);
  Uint8List? get myAvatarBytes => avatars?.myBytes;

  bool _avatarEligible(String peerId) {
    final c = contactFor(peerId);
    return c != null && c.saved && !isBlocked(peerId) && peerId != _nodePeerId;
  }

  Future<void> _sendAvatarText(String peerId, String text) async {
    await _ws?.sendMessage(peerId, text);
  }

  /// Сообщить свой хэш: [only] — этим контактам, иначе всем сохранённым, кто сейчас в сети и кому ещё не сообщали в этом сеансе.
  /// Пустой хэш (картинку убрали) шлётся только с [force].
  void _announceAvatar({Iterable<String>? only, bool force = false}) {
    final a = avatars;
    if (a == null || _ws == null) return;
    final hash = a.myHash;
    if (hash == null && !force) return;
    final targets = (only ?? contacts.where((c) => c.online).map((c) => c.peerId)).toList();
    for (final p in targets) {
      if (!_avatarEligible(p)) continue;
      if (!force && _avatarAnnounced.contains(p)) continue;
      _avatarAnnounced.add(p);
      unawaited(_sendAvatarText(p, AvatarMsg.announce(hash ?? '')));
    }
  }

  /// Раз в 6 часов заново сверяем хэши со всеми, кто в сети (картинка могла смениться, пока мы не виделись).
  void _resyncAvatarsIfDue() {
    if (DateTime.now().difference(_lastAvatarSync) > const Duration(hours: 6)) {
      _avatarAnnounced.clear();
      _lastAvatarSync = DateTime.now();
    }
  }

  Future<void> _onAvatarMsg(String from, AvatarMsg m) async {
    final a = avatars;
    if (a == null || !_avatarEligible(from)) return;   // только от сохранённых контактов
    switch (m.kind) {
      case 'h':
        await a.onAnnounce(from, m.hash, (t) => _sendAvatarText(from, t));
      case 'q':
        await a.onRequest(from, m.hash, (t) => _sendAvatarText(from, t));
      case 'a':
        await a.onImage(from, m.hash, m.png!, decodes: _decodesAsImage);
    }
  }

  static Future<bool> _decodesAsImage(Uint8List png) async {
    try {
      final codec = await ui.instantiateImageCodec(png, targetWidth: 64);
      final frame = await codec.getNextFrame();
      frame.image.dispose();
      codec.dispose();
      return true;
    } catch (_) {
      return false;
    }
  }

  /// Своя картинка из фото (метаданные не переносятся: картинка собирается заново) или из эмодзи. null — готово, иначе текст ошибки.
  Future<String?> setMyAvatarFromImage(Uint8List bytes) async {
    try {
      await avatars!.setMine(await AvatarProcessor.fromImageBytes(bytes));
      _avatarAnnounced.clear();
      _announceAvatar();
      return null;
    } catch (e) {
      return 'Не удалось подготовить картинку: ${e.toString().split('\n').first}';
    }
  }

  Future<String?> setMyAvatarEmoji(String emoji) async {
    try {
      await avatars!.setMine(await AvatarProcessor.fromEmoji(emoji));
      _avatarAnnounced.clear();
      _announceAvatar();
      return null;
    } catch (e) {
      return 'Не удалось подготовить картинку: ${e.toString().split('\n').first}';
    }
  }

  Future<void> clearMyAvatar() async {
    await avatars?.clearMine();
    _avatarAnnounced.clear();
    _announceAvatar(force: true);   // сообщить контактам, что картинки больше нет
  }

  Future<void> blockContact(String peerId, String name) async {
    unawaited(avatars?.forgetPeer(peerId) ?? Future<void>.value());
    _blocked[peerId] = name;
    contacts.removeWhere((c) => c.peerId == peerId);   // исчезает из списка
    _unread.remove(peerId);
    await StorageService.saveBlacklist(_blocked);
    await StorageService.saveUnread(_unread);
    notifyListeners();
  }

  Future<void> unblockContact(String peerId) async {
    _blocked.remove(peerId);
    await StorageService.saveBlacklist(_blocked);
    unawaited(refreshContacts());
    notifyListeners();
  }

  int unreadFor(String peerId) => _unread[peerId] ?? 0;

  void _bumpUnread(String peerId) {
    _unread[peerId] = (_unread[peerId] ?? 0) + 1;
    unawaited(StorageService.saveUnread(_unread));
    notifyListeners();
  }

  void _clearUnread(String peerId) {
    if ((_unread[peerId] ?? 0) == 0) return;
    _unread[peerId] = 0;
    unawaited(StorageService.saveUnread(_unread));
    notifyListeners();
  }

  int _statusRank(MessageStatus s) => switch (s) {
    MessageStatus.failed    => -1,
    MessageStatus.pending   => 0,
    MessageStatus.delivered => 1,
    MessageStatus.read      => 2,
  };

  /// Разобрать входящий текст. Служебное (квитанция/пинг) обрабатываем здесь и возвращаем null; обычное сообщение —
  /// (текст для показа, remoteId|null). remoteId не null, если пришёл конверт с id (значит собеседник ждёт квитанций).
  (String, String?)? _classifyIncoming(String from, String rawText) {
    final av = AvatarMsg.tryParse(rawText);
    if (av != null) { unawaited(_onAvatarMsg(from, av)); return null; }
    if (AvatarMsg.isAvatar(rawText)) return null;   // повреждённый служебный конверт аватарки — не показываем как текст
    final rcpt = MsgChannel.parseReceipt(rawText);
    if (rcpt != null) { _applyReceipt(from, rcpt.$1, rcpt.$2); return null; }
    final edit = MsgChannel.parseEdit(rawText);
    if (edit != null) { _applyEdit(from, edit.$1, edit.$2); return null; }
    if (MsgChannel.isCapPing(rawText)) { _receiptCapable.add(from); return null; }
    final env = MsgChannel.parseMessage(rawText);
    if (env != null) { _receiptCapable.add(from); return (env.$2, env.$1); } // (cmid, text) -> (text, cmid)
    return (rawText, null);
  }

  void _applyReceipt(String from, String kind, List<String> ids) {
    final list = _chats[from];
    if (list == null) return;
    final target = kind == 'read' ? MessageStatus.read : MessageStatus.delivered;
    var changed = false;
    for (var i = 0; i < list.length; i++) {
      final m = list[i];
      if (m.outgoing && ids.contains(m.id) && _statusRank(m.status) < _statusRank(target)) {
        list[i] = m.copyWith(status: target);
        unawaited(StorageService.updateMessageStatus(m.id, target));
        changed = true;
      }
    }
    if (changed) notifyListeners();
  }

  void _sendDeliveredReceipt(String to, String cmid) {
    unawaited(_ws?.sendMessage(to, MsgChannel.receipt('delivered', [cmid])) ?? Future<bool>.value(false));
  }

  /// Объявить своим устройствам, что эта сборка умеет квитанции (пинг «вживую», старые сборки его игнорируют).
  void announceReceiptSupport() {
    for (final c in contacts) {
      if (!c.isManual) { // устройство владельца, не ручной контакт
        unawaited(_ws?.sendLive(c.peerId, MsgChannel.capPing()) ?? Future<bool>.value(false));
      }
    }
  }

  /// Чат открыт/дошли новые — отправить «прочитано» по входящим этого собеседника.
  void markChatRead(String peerId) {
    _clearUnread(peerId);
    final list = _chats[peerId];
    if (list == null) return;
    final ids = <String>[];
    for (final m in list) {
      final rid = m.remoteId;
      if (!m.outgoing && rid != null && !_readAcked.contains(rid)) {
        ids.add(rid);
        _readAcked.add(rid);
      }
    }
    if (ids.isNotEmpty) {
      unawaited(_ws?.sendMessage(peerId, MsgChannel.receipt('read', ids)) ?? Future<bool>.value(false));
    }
  }

  /// Удалить выбранные сообщения — только у себя (у собеседника остаются; у него тоже только ручное удаление).
  Future<void> deleteMessages(String peerId, Set<String> ids) async {
    final list = _chats[peerId];
    if (list != null) list.removeWhere((m) => ids.contains(m.id));
    await StorageService.deleteMessages(ids.toList());
    notifyListeners();
  }

  /// Очистить переписку с собеседником (локально).
  Future<void> clearChat(String peerId) async {
    _chats[peerId]?.clear();
    await StorageService.clearChat(peerId);
    notifyListeners();
  }

  Future<void> sendMessage(String peerId, String text) async {
    if (isBlocked(peerId)) return;   // двусторонний блок: заблокированному не отправляем
    final id = DateTime.now().millisecondsSinceEpoch.toString();
    // Всегда конверт с id: получатель по нему отвечает «доставлено/прочитано». Раньше обёртка зависела от того, успел ли собеседник ответить
    // на пинг поддержки в этом сеансе: после запуска приложения это знание пусто, и всё отправленное в это окно уходило без номера — без галочек.
    // Шифровать мы можем только устройствам со связкой ключей E4, то есть новым сборкам, а они квитанции умеют.
    final payload = MsgChannel.wrapMessage(id, text);
    final sent = await (_ws?.sendMessage(peerId, payload) ?? Future<bool>.value(false));

    final msg = ChatMessage(
      id:        id,
      peerId:    peerId,
      outgoing:  true,
      text:      text,
      timestamp: DateTime.now(),
      status:    sent ? MessageStatus.pending : MessageStatus.failed,
    );
    _chats.putIfAbsent(peerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    notifyListeners();
  }

  /// Последний запрос о прокси узла не удался (узла нет на связи): тогда «не запущен» говорить нельзя.
  bool proxyUnreachable = false;

  Future<void> refreshProxyInfo() async {
    try {
      final info = await _api!.getProxyInfo();
      proxyUnreachable = false;
      if (info['running'] == true) {
        proxyHost = info['host'] as String?;
        proxyPort = info['port'] as int?;
      } else {
        proxyHost = null;
        proxyPort = null;
      }
      notifyListeners();
    } catch (_) {
      proxyUnreachable = true;
      notifyListeners();
    }
  }

  // ── VPN ───────────────────────────────────────────────────────────────────

  Future<void> toggleVpn() async {
    if (vpnRunning) {
      await vpn.stop();
      vpnRunning = false;
      notifyListeners();
      return;
    }
    final node = nodeManager.activeNode;
    if (node == null || node.token == null) return;
    final ok = await vpn.start(
      host: node.host,
      port: node.port,
      fingerprint: node.fingerprint,
      token: node.token!,
    );
    // сервис стартует асинхронно: проверяем, что он правда поднялся, а не верим первому «да»
    var up = false;
    if (ok) {
      for (var i = 0; i < 10 && !up; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 300));
        up = await vpn.isVpnRunning();
      }
    }
    vpnRunning = up;
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
    if (isBlocked(e.fromPeerId)) return;   // чёрный список: не показываем и не подтверждаем
    final res = _classifyIncoming(e.fromPeerId, e.text);
    if (res == null) { notifyListeners(); return; } // служебное (квитанция/пинг) — не показываем
    final showText = res.$1;
    final remoteId = res.$2;
    final msg = ChatMessage(
      id:        'inbox_${e.timestamp.millisecondsSinceEpoch}',
      peerId:    e.fromPeerId,
      outgoing:  false,
      text:      showText,
      timestamp: e.timestamp,
      status:    MessageStatus.delivered,
      remoteId:  remoteId,
    );
    _chats.putIfAbsent(e.fromPeerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    if (activeChatPeerId != e.fromPeerId) _bumpUnread(e.fromPeerId);
    if (remoteId != null) {
      _sendDeliveredReceipt(e.fromPeerId, remoteId);
      // чат открыт — сразу «прочитано»
      if (activeChatPeerId == e.fromPeerId && !_readAcked.contains(remoteId)) {
        _readAcked.add(remoteId);
        unawaited(_ws?.sendMessage(e.fromPeerId, MsgChannel.receipt('read', [remoteId])) ?? Future<bool>.value(false));
      }
    }

    if (activeChatPeerId != e.fromPeerId) {
      _player.play(AssetSource('sounds/icq.mp3'));
      // Уведомление когда чат не открыт
      final name = _contactName(e.fromPeerId);
      unawaited(NotificationService.showMessage(
        fromPeerId:  e.fromPeerId,
        displayName: name,
        text:        FileOffer.preview(showText),
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
    if (e.online) _announceAvatar(only: [e.peerId]);   // контакт появился — сверяем картинки (хэши)
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

  /// Блокировка приложения: связь с узлом закрывается, всё расшифрованное из памяти убирается, база закрывается.
  /// (Уведомления о новых сообщениях при этом приходят: служба в фоне ничего не расшифровывает; почта ждёт на узле до входа.)
  Future<void> lockDown() async {
    _presenceTimer?.cancel();
    _ws?.dispose();
    _ws = null;
    _api?.dispose();
    _api = null;
    nodeOnline = false;
    nodeError = null;
    identity = null;
    groups = null;
    avatars = null;
    _vaultFiles = null;
    VaultBytesCache.clear();
    contacts = [];
    nodeManager.clear();
    _chats.clear();
    _unread.clear();
    _blocked.clear();
    _editedIds.clear();
    _readAcked.clear();
    pendingFileOffers.clear();
    activeChatPeerId = null;
    notifyListeners();
  }

  /// «Удалить всё»: службы останавливаются, память очищается, затем стираются ключи, база, файлы и секреты.
  Future<void> eraseEverything() async {
    try { await chatBg.stop(); } catch (_) {}
    await lockDown();
    await VaultService.instance.eraseEverything();
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
