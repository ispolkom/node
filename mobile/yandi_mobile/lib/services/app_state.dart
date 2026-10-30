import '../models/support_wallet.dart';
import 'prep_tracker.dart';
import 'invite_store.dart';
import '../app_version.dart';
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
import 'app_sounds.dart';
import 'contact_cache.dart';
import '../models/trusted_node.dart';
import '../models/invite_payload.dart';
import '../models/contact.dart';
import '../models/contact_merge.dart';
import '../models/group_invite.dart';
import '../models/contact_card.dart';
import '../models/message.dart';
import '../models/tombstone.dart';
import '../models/file_offer.dart';
import '../models/file_state.dart';
import '../models/receipt.dart';
import '../models/avatar_msg.dart';
import 'avatar_service.dart';
import 'avatar_processor.dart';
import '../crypto/identity.dart';
import 'outbox.dart';
import 'storage_service.dart';
import '../crypto/file_vault.dart';
import 'vault_bytes_cache.dart';
import 'vault_files.dart';
import 'vault_service.dart';
import 'video_frames.dart';
import 'api_service.dart' as api_svc;
import '../content_policy.dart';
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
import 'package:flutter/material.dart' show MaterialPageRoute, WidgetsBinding, AppLifecycleState;
import 'package:flutter/painting.dart' show PaintingBinding;
import '../l10n_core.dart';

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
  final soundSettings = <String, bool>{for (final key in AppSounds.assets.keys) key: true};
  bool soundEnabled(String kind) => soundSettings[kind] ?? true;

  Future<void> setSoundEnabled(String kind, bool enabled) async {
    if (!AppSounds.assets.containsKey(kind)) return;
    soundSettings[kind] = enabled;
    _calls?.refreshRingtone();
    notifyListeners();
    await StorageService.secureWrite('pref_sound_$kind', enabled ? '1' : '0');
    if (kind == 'messages') await chatBg.setMessageSoundEnabled(enabled);
  }

  Future<void> playAppSound(String kind) async {
    if (!soundEnabled(kind) || _calls?.busy == true) return;
    try {
      await _player.play(AssetSource('sounds/${AppSounds.assets[kind]}'), ctx: AppSounds.context(kind));
    } catch (_) {
      debugPrint('YANDI: notification sound unavailable');
    }
  }

  /// Входящие предложения файлов (для отображения в UI)
  final List<FileOfferEvent> pendingFileOffers = [];

  // ── Accessors ──────────────────────────────────────────────────────────────
  TrustedNode? get activeNode  => nodeManager.activeNode;
  bool         get isPaired    => nodeManager.rankedNodes.isNotEmpty;
  String?      get myPeerId    => identity?.peerId;
  api_svc.ApiService   get apiService  => _api!;
  /// Каталог «Узлы» страницы поиска; null — нет связи с узлом.
  Future<List<Map<String, dynamic>>?> nodeDirectory() => _api?.getNodes() ?? Future.value(null);

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
    for (final kind in AppSounds.assets.keys) {
      soundSettings[kind] = (await StorageService.secureRead('pref_sound_$kind')) != '0';
    }
    await chatBg.setMessageSoundEnabled(soundEnabled('messages'));
    await _loadHidden();
    await _loadProfileMeta();
    await _loadRx();
    await _loadForgotten();

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

    // Профиль (ник и картинка) теперь у каждого устройства свой, а не общий с ПК и другими телефонами. Один раз сбрасываем
    // ранее подтянутый общий профиль, чтобы у всех устройств не оказалось одинаковой картинки; человек задаёт свой.
    try {
      if (await StorageService.secureRead('profile_v2') != '1') {
        myNick = '';
        await av.clearMine();
        _profileDirty = false;
        await _saveProfileMeta();
        await StorageService.secureWrite('nick_prompted', '0');
        await StorageService.secureWrite('profile_v2', '1');
      }
    } catch (_) {}

    unawaited(_filesHousekeeping());
    unawaited(StorageService.replayLedger?.reseal().catchError((_) {}) ?? Future<void>.value());   // старый открытый журнал повторов — в зашифрованный вид

    // 2. Ключи пользователя (генерируются один раз и навсегда)
    identity = await Identity.loadOrGenerate();

    // 3. Список нод
    await nodeManager.load();
    // Restore people before any network request, including an offline unlock.
    await _restoreContacts(nodeManager.activeNode?.id ?? 'local');

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
      nodeError = trf('Узел {n}: нет отпечатка сертификата. Сопрягите телефон с узлом заново (QR). ({e})', {'n': node.name, 'e': e});
      notifyListeners();
      return;
    }
    nodeError = null;
    groups = GroupService(HttpGroupApi(_api!), SqliteGroupStore(), onChanged: (_) => notifyListeners(), onActivity: (_) => unawaited(playAppSound('groups')), nodeKey: node.id);
    _ws  = WsService(
      identity!,
      onPermanentLoss: _onConnectionLost,
      onSignalLoss: () => calls.onSignalingLost(),
      onOpen: () {
        // сокет открылся (первый раз или после обрыва): статусы за время обрыва не приходили, берём заново, и забираем накопленную почту
        unawaited(refreshContacts());
        unawaited(_syncInbox());
        _outbox.connectionBack(DateTime.now());
        unawaited(retryUnsent());   // связь вернулась: неотправленное уходит сразу
        announceReceiptSupport();
        unawaited(syncGroups());
        unawaited(syncForgotten());   // за время обрыва могли убрать устройства
        unawaited(pruneExpired());    // сроки хранения: база уже открыта
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
    _ws!.profileStream.listen((_) => unawaited(syncProfile()));   // узел сообщил, что профиль изменился
    _ws!.forgottenStream.listen((_) => unawaited(syncForgotten()));   // убраны устройства: их сообщения становятся надгробиями

    chatBg.start(node);

    // неотправленные сообщения повторяются сами (паузы и срок — в Outbox)
    _retryTimer?.cancel();
    _retryTimer = async_lib.Timer.periodic(const Duration(seconds: 5), (_) => unawaited(retryUnsent()));

    // статусы «в сети» ещё и подтягиваем раз в 20 секунд: события о смене могут потеряться при обрыве связи
    _presenceTimer?.cancel();
    _presenceTimer = async_lib.Timer.periodic(const Duration(seconds: 20), (_) {
      unawaited(refreshContacts());
      unawaited(syncProfile());
      unawaited(refreshRole());
    });

    unawaited(checkAppUpdate());
    _updateTimer?.cancel();
    _updateTimer = async_lib.Timer.periodic(const Duration(hours: 6), (_) => unawaited(checkAppUpdate()));

    // срок жизни сообщений — 365 суток: чистка раз в сутки, пока приложение работает (и при каждом открытии сокета)
    _pruneTimer?.cancel();
    _pruneTimer = async_lib.Timer.periodic(const Duration(days: 1), (_) => unawaited(pruneExpired()));

    unawaited(_loadInitialData());
  }

  async_lib.Timer? _presenceTimer;
  async_lib.Timer? _pruneTimer;

  // ── Новая версия приложения ───────────────────────────────────────────────
  /// {version, notes, size, page} — есть версия новее этой; null — нет или человек отложил это сообщение.
  Map<String, dynamic>? appUpdate;

  /// Кошелёк проекта из подписанного манифеста узла; заменяет вшитый (адрес меняется одной строкой манифеста).
  List<SupportWallet> signedWallets = const [];
  List<SupportWallet> get wallets => signedWallets.isNotEmpty ? signedWallets : supportWallets;
  String? _updateDismissed;
  async_lib.Timer? _updateTimer;

  Future<void> checkAppUpdate() async {
    final ans = await _api?.getUpdateAnswer(kAppVersion);
    final av = ans?['available'];
    final a = av is Map<String, dynamic> ? av : null;
    final w = SupportWallet.listFromSigned(ans?['support']);
    if (w.isNotEmpty && w.map((x) => '${x.id}|${x.address}').join(',') != signedWallets.map((x) => '${x.id}|${x.address}').join(',')) {
      signedWallets = w;
      notifyListeners();
    }
    final next = (a != null && a['version'] != _updateDismissed) ? a : null;
    if (next?['version'] != appUpdate?['version']) {
      appUpdate = next;
      notifyListeners();
    }
  }

  void dismissAppUpdate() {
    _updateDismissed = appUpdate?['version'] as String?;
    appUpdate = null;
    notifyListeners();
  }

  // ── Автоповтор неотправленных сообщений ──────────────────────────────────
  final Outbox _outbox = Outbox();
  async_lib.Timer? _retryTimer;
  bool _retrying = false;

  /// Сколько неотправленных сообщений сейчас ждёт повтора (для показателя стабильности связи).
  int get unsentWaiting => _outbox.waiting;

  /// Один проход: неотправленные личные сообщения уходят снова тем же номером (получатель отбрасывает повтор по номеру).
  Future<void> retryUnsent() async {
    final ws = _ws;
    if (_retrying || ws == null || !ws.connected || appRevoked) return;
    _retrying = true;
    try {
      final now = DateTime.now();
      final failed = await StorageService.failedOutgoing(
        sinceMs: now.subtract(Outbox.maxAge).millisecondsSinceEpoch,
        skipPrefixes: [FileOffer.marker, GroupInviteMsg.marker, ContactCardMsg.marker],
        skipPeers: _blocked.keys.toList(),
      );
      _outbox.keepOnly({for (final m in failed) m.id});
      for (final m in _outbox.due(failed, now, skip: (m) => isBlocked(m.peerId))) {
        if (_ws != ws || !ws.connected) break;
        // состояние могло измениться, пока шёл предыдущий await: собеседника заблокировали, сообщение удалили или оно уже подтверждено
        if (isBlocked(m.peerId) || await StorageService.outgoingStatus(m.id) != MessageStatus.failed) continue;
        if (!await _guestSendAllowed()) break;   // лимит гостя считает и повторы
        final payload = MsgChannel.wrapMessage(m.id, m.text, replyId: m.replyId, replyText: m.replyText);
        final ok = await ws.sendMessage(m.peerId, payload);
        if (!ok) {
          await _guestSendRefund();
          _outbox.failedAgain(m.id, DateTime.now());
          continue;
        }
        _outbox.sent(m.id);
        // квитанция, пришедшая за время отправки, не понижается: обновляем только если оно всё ещё failed
        if (await StorageService.markPendingIfFailed(m.id)) {
          final cur = _chats[m.peerId]?.where((x) => x.id == m.id).firstOrNull;
          if (cur != null && cur.status == MessageStatus.failed) _replaceMessage(m.peerId, m.id, cur.copyWith(status: MessageStatus.pending));
          notifyListeners();
        }
      }
      // «ушло в сокет, а квитанции нет»: пакет мог пропасть при обрыве; повторяем тем же номером (получатель с квитанциями отбросит повтор)
      final pending = await StorageService.pendingOutgoing(sinceMs: now.subtract(Outbox.maxAge).millisecondsSinceEpoch);
      _outbox.forgetPending({for (final m in pending) m.id});
      for (final m in _outbox.duePending(pending, now, capable: (m) => _receiptCapable.contains(m.peerId) && !isBlocked(m.peerId))) {
        if (_ws != ws || !ws.connected) break;
        if (isBlocked(m.peerId) || await StorageService.outgoingStatus(m.id) != MessageStatus.pending) continue;
        if (!await _guestSendAllowed()) break;
        _outbox.pendingResent(m.id);
        if (!await ws.sendMessage(m.peerId, MsgChannel.wrapMessage(m.id, m.text, replyId: m.replyId, replyText: m.replyText))) await _guestSendRefund();
      }
    } catch (_) {
      // база или сеть недоступны: попробуем на следующем проходе
    } finally {
      _retrying = false;
    }
  }

  /// Группы узла (ключи, журнал, правила). Пересоздаётся вместе с подключением к узлу.
  GroupService? groups;

  /// Для экранов: перерисоваться после действия над группами.
  void notifyListenersPublic() => notifyListeners();

  /// Скрытые на странице поиска пользователи и группы (только у меня на телефоне).
  final Set<String> hiddenUsers = {};
  final Set<String> hiddenGroups = {};

  Future<void> _loadHidden() async {
    for (final (key, set) in [('hidden_users', hiddenUsers), ('hidden_groups', hiddenGroups)]) {
      try {
        final raw = await StorageService.secureRead(key);
        if (raw != null && raw.isNotEmpty) set.addAll((jsonDecode(raw) as List).cast<String>());
      } catch (_) {}
    }
  }

  Future<void> setHidden({required bool group, required String id, required bool hidden}) async {
    final set = group ? hiddenGroups : hiddenUsers;
    hidden ? set.add(id) : set.remove(id);
    await StorageService.secureWrite(group ? 'hidden_groups' : 'hidden_users', jsonEncode(set.toList()));
    notifyListeners();
  }

  Future<void> syncGroups({String? only}) async {
    final g = groups;
    if (g == null) return;
    g.setProfile(myNick, avatars?.myBytes);   // визитка в группах берётся из профиля
    final id = identity;
    if (id != null) {
      g.signer = id.sign;
      g.contactAvatar = (p) => (avatars?.hasLocal(p) ?? false) ? avatars!.bytesForPeer(p) : null;
      g.contactKeys = () async {
        // ключ подписи контакта — тот, что закреплён при переписке (peer_ed25519_pin_*); нет закреплённого — контакт ещё не писал
        final out = <(List<int>, String, String)>[];
        for (final c in contacts) {
          if (!c.saved) continue;
          final pin = await StorageService.secureRead('peer_ed25519_pin_${c.peerId}');
          if (pin == null) continue;
          try { out.add((base64.decode(pin), c.displayName, c.peerId)); } catch (_) {}
        }
        return out;
      };
    }
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

  /// Роль и право приглашать могут измениться на узле (администратор подтвердил устройство): подтягиваем их вместе с остальным, не дожидаясь переподключения.
  Future<void> refreshRole() async {
    try {
      final info = await _api?.getInfo();
      if (info == null) return;
      final role = info['role'] as String?;
      final can = info['can_invite'] == true;
      _noteMinApp(info['min_app']);
      if (role != myRole || can != canInvite) {
        myRole = role;
        canInvite = can;
        _syncRoleFlags();
        notifyListeners();
      }
      if (role == 'guest') await refreshStatusRequest();
    } catch (_) {}
  }

  /// Отзыв сборок: версия приложения ниже `min_app` узла (из подписанного списка) — жёсткая блокировка отправки и обязательный экран
  /// обновления на главной. Снимается, только когда узел перестал требовать (или приложение обновили).
  String? requiredApp;
  bool get appRevoked => requiredApp != null;

  void _noteMinApp(Object? v) {
    final m = v is String && v.isNotEmpty && versionOlder(kAppVersion, v) ? v : null;
    if (m == requiredApp) return;
    requiredApp = m;
    if (m != null) nodeError = trf('Эта версия приложения ({v}) отозвана: обновите приложение до {m} или новее.', {'v': kAppVersion, 'm': m});
    notifyListeners();
  }

  Future<void> _loadInitialData() async {
    unawaited(_initDownloadsDir());
    try {
      final info = await _api!.getInfo();
      nodeOnline = info['online'] as bool? ?? false;
      _nodePeerId = info['node_id'] as String?;   // сам компьютер — не «человек»: аватарки ему не шлём
      // отзыв сборок (docs/ROADMAP_R9.md §5): эта версия приложения ниже минимальной по подписанному списку узлов
      _noteMinApp(info['min_app']);
      myRole = info['role'] as String?;
      canInvite = info['can_invite'] == true;
      _syncRoleFlags();
      if (isGuest) unawaited(refreshStatusRequest());

      // Регистрируем свои публичные ключи на ноде (для E2E входящих)
      if (identity != null) {
        await _api!.registerPublicKeys(
          ed25519PubBase64: identity!.ed25519PubBase64,
          x25519PubBase64:  identity!.x25519PubBase64,
          signatureBase64:   await identity!.keyBundleSignatureBase64(),
        );
      }

      await refreshContacts();
      unawaited(syncProfile());
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
          nodeError = tr('В очереди есть старые неподписанные E2/E3 сообщения. Они сохранены на узле; нужен согласованный перенос.');
          notifyListeners();
          continue;
        }

        // Как и в сокете: только E2E-конверт, общее окно повторов. Открытый текст, старый формат, подделку или повтор (то же
        // сообщение уже пришло вживую) не показываем, но подтверждаем, чтобы узел не присылал его снова.
        var duplicate = false;
        // номер письма занимается ПОСЛЕ сохранения: здесь только проверка «не виденное» и запоминание номера
        String? pendingReplayId;
        int pendingReplayExp = 0;
        final ledger = StorageService.replayLedger;
        final text = await ws.decryptIncoming(
          payloadBytes,
          fromPeerId: fromPeerId,
          onDuplicate: () => duplicate = true,
          claimOverride: ledger == null ? null : (rid, exp) async {
            if (await ledger.seen(rid)) return false;
            pendingReplayId = rid;
            pendingReplayExp = exp;
            return true;
          },
        );
        if (text == null) {
          // Only a verified persisted replay may be ACKed. Missing keys, offline
          // info or disk errors must not silently delete valid queued mail.
          if (duplicate) ackIds.add(id);
          continue;
        }

        // фиксация номера письма после обработки; не записалась — письмо остаётся на узле и придёт снова (сохранение идемпотентно)
        Future<bool> commitReplay() async {
          final rid = pendingReplayId;
          if (rid == null || ledger == null) return true;
          try {
            await ledger.claim(rid, pendingReplayExp);
            return true;
          } catch (_) {
            return false;
          }
        }
        if (isBlocked(fromPeerId)) { if (await commitReplay()) ackIds.add(id); continue; }   // чёрный список
        final res = _classifyIncoming(fromPeerId, text);
        if (res == null) { if (await commitReplay()) ackIds.add(id); continue; } // служебное — подтверждаем и пропускаем
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
          replyId:   res.$3,
          replyText: res.$4,
        );
        // запись и проверка «не было ли уже» — одна операция базы; повтор отправителя подтверждаем, но второй раз не показываем
        final fresh = await StorageService.insertIncomingIfNew(msg);
        if (!fresh) {
          if (await commitReplay()) {
            if (remoteId != null) _sendDeliveredReceipt(fromPeerId, remoteId);
            ackIds.add(id);
          }
          continue;
        }
        if (remoteId != null) { _rids[msg.id] = remoteId; _saveRx(); }
        if (activeChatPeerId != fromPeerId) _bumpUnread(fromPeerId);
        if (!await commitReplay()) continue;   // сохранено, но номер не занят: подтверждение — в следующий проход
        if (remoteId != null) _sendDeliveredReceipt(fromPeerId, remoteId);
        ackIds.add(id);
      }

      if (ackIds.isNotEmpty) await _api!.ackInbox(ackIds);
    } catch (_) {}
  }

  // ── Паринг новой ноды ──────────────────────────────────────────────────────

  Future<void> completePairing(TrustedNode node) async {
    // Сопряжение по QR — личное действие у экрана узла: закреплённый ключ этого узла забывается, чтобы после пересоздания ключей
    // узла (новая среда, перенос) не оставаться с устаревшим ключом и не отвергать его сообщения.
    // старая запись этого же узла на этом телефоне: её устройство на узле отзывается, иначе в «Пользователях» будет двойник
    for (final old in nodeManager.nodes.where((n) => n.id == node.id && n.token != node.token).toList()) {
      await api_svc.ApiService.revokeSelf(old);
    }
    await nodeManager.addNode(node);
    // забываем только после того, как новое сопряжение сохранено: сбой записи не оставляет старое сопряжение без закреплённого ключа
    if (node.id.length == 64) {
      try { await StorageService.secureDelete('peer_ed25519_pin_${node.id}'); } catch (_) {}
    }

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
    _contactsRevision++;
    final existing = contactFor(peerId);
    final clean = cleanPersonName(name);
    final shown = clean.isNotEmpty
        ? clean
        : (existing != null ? _plainName(existing.displayName) : '');
    final finalName = shown.isNotEmpty ? shown : peerId.substring(0, 12);
    await StorageService.saveManualContact(peerId, finalName);
    // общий список узла: имя увидят ПК и другие телефоны этого же владельца (свои устройства и сам узел туда не попадают)
    if (peerId.length == 64 && peerId != _nodePeerId && (existing == null || existing.isManual)) {
      unawaited(() async {
        try {
          await _api?.json('POST', '/mobile/contacts', {'peer_id': peerId, 'name': finalName});
        } catch (_) {}
      }());
    }
    if (existing != null) {
      contacts[contacts.indexOf(existing)] = Contact(
        peerId: peerId, displayName: finalName, online: existing.online,
        isManual: existing.isManual, saved: true, lastActivityMs: existing.lastActivityMs, role: existing.role,
      );
    } else {
      contacts.add(Contact(peerId: peerId, displayName: finalName, online: false, isManual: true));
    }
    notifyListeners();
    _announceAvatar(only: [peerId]);   // добавили в контакты — сообщаем свою картинку
    await _contactCache.save(_api?.nodeId ?? activeNode?.id ?? 'local', contacts);
  }

  /// Убрать из контактов. Контакты, заведённые на самом узле, остаются (их удаляют там).
  Future<void> removeManualContact(String peerId) async {
    _contactsRevision++;
    await StorageService.deleteManualContact(peerId);
    unawaited(avatars?.forgetPeer(peerId) ?? Future<void>.value());   // не в контактах — картинку не показываем и не храним
    _avatarAnnounced.remove(peerId);
    final c = contactFor(peerId);
    // удалённый уходит со вкладки «Контакты» на вкладку поиска («не в контактах»), а не пропадает совсем
    if (c != null) {
      contacts[contacts.indexOf(c)] = c.copyWith(saved: false);
    }
    notifyListeners();
    await _contactCache.save(_api?.nodeId ?? activeNode?.id ?? 'local', contacts);
  }

  Future<void> unpairNode(String nodeId) async {
    // с каким узлом соединены на самом деле — запоминаем ДО удаления: вычисляемый «активный» после удаления уже другой, и без
    // этого соединение с удалённым узлом (и его токен) продолжали бы использоваться
    final connectedHere = _api?.nodeId == nodeId || activeNode?.id == nodeId;
    await nodeManager.removeNode(nodeId);

    // Если удалили активную — переключаемся на следующую
    if (connectedHere || activeNode == null) {
      final next = await nodeManager.selectBest();
      if (next != null) {
        _switchTo(next);
      } else {
        _ws?.disconnect();
        chatBg.stop();
        nodeOnline = false;
      }
    }
    notifyListeners();
  }

  /// Единственный способ сменить узел у живых служб. Сервис групп привязан к номеру узла (список «известных групп» у каждого свой),
  /// поэтому при смене он пересоздаётся: старый, оставшись с чужим списком, принял бы группы прежнего узла за удалённые и стёр ключи.
  void _switchTo(TrustedNode next) {
    _deviceProfilePublished = false;   // профиль публикуется на каждом узле отдельно
    _lastUsers = const [];             // люди прежнего узла новому не принадлежат
    _dataGen++;                        // и ответы, начатые у прежнего узла, не принимаются
    contacts = [];
    unawaited(_restoreContacts(next.id));
    groups?.retire();
    _api?.switchNode(next);
    _ws?.switchNode(next);
    if (_api != null) {
      groups = GroupService(HttpGroupApi(_api!), SqliteGroupStore(), onChanged: (_) => notifyListeners(), onActivity: (_) => unawaited(playAppSound('groups')), nodeKey: next.id);
    }
    chatBg.start(next);
  }

  // ── Failover ───────────────────────────────────────────────────────────────

  Future<void> _onConnectionLost() async {
    final next = await nodeManager.failover(currentId: _api?.nodeId);
    if (next != null) {
      _switchTo(next);
      unawaited(_syncInbox());
    }
  }

  // ── Данные ────────────────────────────────────────────────────────────────

  /// Поколение данных: растёт при смене узла и блокировке; ответ, начатый в прошлом поколении, в списки не кладётся.
  int _dataGen = 0;
  int _contactsRevision = 0;

  final _contactCache = ContactCache(read: StorageService.secureRead, write: StorageService.secureWrite);

  Future<void> _restoreContacts(String nodeId) async {
    final gen = _dataGen;
    final revision = _contactsRevision;
    final saved = {for (final r in await StorageService.loadManualContacts())
      r['peer_id'] as String: (r['name'] as String?) ?? ''};
    final local = await _contactCache.load(nodeId, saved, isBlocked);
    if (gen != _dataGen || revision != _contactsRevision) return;
    contacts = local;
    notifyListeners();
  }

  Future<void> refreshContacts() async {
    try {
      final api = _api!;
      final gen = _dataGen;
      final nodeId = api.nodeId;
      // три источника читаются независимо: сбой одного (таймаут, чужой ответ) не прячет людей из остальных
      List<Contact>? fromNode;
      try { fromNode = await api.getContacts(); } catch (_) {}
      Map<String, String> saved = const {};
      try {
        saved = {
          for (final r in await StorageService.loadManualContacts())
            r['peer_id'] as String: (r['name'] as String?) ?? '',
        };
      } catch (_) {}
      // не владелец (гость, пользователь узла): список людей даёт каталог узла (все, в сети и не в сети, с ролью, ником, картинкой)
      if (myRole != 'owner') {
        final fresh = await api.getUsers();
        if (gen != _dataGen) return;
        _lastUsers = fresh ?? [for (final u in _lastUsers) {...u, 'online': false}];
      }
      if (gen != _dataGen) return;   // за время ответа узел сменился или приложение заблокировано
      _contactsRevision++;
      contacts = mergeContacts(
        fromNode: fromNode ?? [for (final c in contacts) c.copyWith(online: false)],
        catalog: myRole != 'owner' ? _lastUsers : const [],
        savedNames: saved,
        isBlocked: isBlocked,
      );
      notifyListeners();
      await _contactCache.save(nodeId, contacts);
      if (gen != _dataGen) return;
      announceReceiptSupport();
      _resyncAvatarsIfDue();
      _announceAvatar();
    } catch (_) {}
  }

  /// Растёт при каждой блокировке: незавершённые асинхронные чтения прежнего сеанса узнают, что их результат уже не нужен.
  int _lockEpoch = 0;

  Future<void> loadChatHistory(String peerId) async {
    // поколение блокировки: ответ запроса, начатого до блокировки, в память не кладётся (иначе расшифрованная история
    // вернулась бы в _chats уже после lockDown)
    final epoch = _lockEpoch;
    try {
      // Сначала из локального SQLite (быстро, без сети)
      final local = await _backfillRids(await StorageService.loadMessages(peerId));
      if (epoch != _lockEpoch) return;
      if (local.isNotEmpty) {
        _chats[peerId] = local;
        notifyListeners();
      }
      // Потом delta с ноды
      if (_api != null && myPeerId != null) {
        final remote = await _api!.getHistory(peerId, myPeerId!);
        if (epoch != _lockEpoch) return;
        final cutoff = DateTime.now().subtract(messageLifetime);
        for (final msg in remote) {
          if (msg.timestamp.isBefore(cutoff)) continue;   // старше срока хранения (365 суток): обратно не кладём
          // устройство убрано с узла: его сообщения из истории узла не оживают и не дублируют надгробия (они уже в базе)
          if (!msg.outgoing && _forgottenPeers.contains(peerId)) continue;
          await StorageService.saveMessage(msg);
        }
        final merged = await _backfillRids(await StorageService.loadMessages(peerId));
        if (epoch != _lockEpoch) return;
        _chats[peerId] = merged;
        notifyListeners();
      }
    } catch (_) {}
  }

  /// Сообщения, полученные до схемы v7, не имеют remote_id в БД, но номер у отправителя сохранён в словаре `_rids`:
  /// подставляем его и дописываем в БД, чтобы «прочитано» и правки работали и для старой переписки.
  Future<List<ChatMessage>> _backfillRids(List<ChatMessage> list) async {
    var out = list;
    for (var i = 0; i < list.length; i++) {
      final m = list[i];
      if (m.outgoing || m.remoteId != null) continue;
      final rid = _rids[m.id];
      if (rid == null) continue;
      if (identical(out, list)) out = List.of(list);
      out[i] = m.copyWith(remoteId: rid);
      try { await StorageService.setRemoteIdIfMissing(m.id, rid); } catch (_) {}
    }
    return out;
  }

  // ── Звонки ─────────────────────────────────────────────────────────────────

  CallService? _calls;
  CallService get calls => _calls ??= CallService(
    soundEnabled: soundEnabled,
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
      return tr('Звонить можно только на свои устройства (у этого собеседника нет ключа шифрования)');
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
  final CopyOwners _copies = CopyOwners();

  /// То же, но не трогает копию, которую сейчас читает операция отправки (её затрёт последняя, когда отпустит).
  Future<void> dropPickerCopyIfFree(String path) async {
    if (_copies.inUse(path)) return;
    await dropPickerCopy(path);
  }

  Future<void> dropPickerCopy(String path) async {
    try {
      final cache = await pp.getTemporaryDirectory();
      // Проверяем границу каталога, чтобы cache-other не считался нашей копией.
      final realCache = await cache.resolveSymbolicLinks();
      final file = File(path);
      if (!await file.exists()) return;
      final realPath = await file.resolveSymbolicLinks();
      if (!realPath.startsWith('$realCache${Platform.pathSeparator}')) return;
      // читатель мог появиться, пока проверялся каталог: тогда копию затрёт последний, кто её отпустит
      if (!_copies.beginWipe(path)) return;
      try {
        if (!await wipeFile(realPath) && await file.exists()) {
          protectionWarning = tr('Не удалось затереть временную копию выбранного файла; попытка повторится при следующем входе.');
          notifyListeners();
        }
      } finally {
        _copies.endWipe(path);
      }
    } catch (_) {
      protectionWarning = tr('Проверка временной копии выбранного файла не завершилась.');
      notifyListeners();
    }
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
    protectionWarning = null;
    try {
      final vf = await vaultFiles();
      await vf.cleanTemp();
      await vf.finishPendingWipes();
      await vf.migrateLegacy(resolveId: _legacyIdFor);
      await _sweepPickerCache();
      final failed = vf.migrationFailures + vf.cleanupFailures + (avatars?.cleanupFailures ?? 0) + StorageService.cleanupFailures;
      if (failed > 0) {
        protectionWarning = '${protectionWarning == null ? '' : '${protectionWarning!} '}${trf('Не все файлы удалось зашифровать или затереть ({n}); попытка повторится при следующем входе.', {'n': failed})}';
      }
    } catch (e) {
      protectionWarning = trf('Проверка защиты файлов не завершилась: {e}', {'e': e});
    }
    notifyListeners();
  }

  /// Показывается в настройках, пока что-то из защиты не доведено до конца (открытые остатки старых файлов и т. п.).
  String? protectionWarning;

  /// Старый открытый файл был скачан в чате или группе: ищем номер передачи, чтобы после шифрования сообщение нашло свой файл.
  Future<String?> _legacyIdFor(String path, String fileName) async {
    try {
      for (final k in await StorageService.secureKeys()) {
        if (k.startsWith('grp_dl_') && await StorageService.secureRead(k) == path) return k.substring(7);   // группа: номер файла на узле
      }
      final m = RegExp(r'^([0-9a-f]{8})_(.*)$').firstMatch(fileName);
      if (m == null) return null;
      final rows = await StorageService.db.query('messages', columns: ['text'], where: 'text LIKE ?', whereArgs: ['%yandi-file:%']);
      final tids = <String>{};
      for (final r in rows) {
        final o = FileOffer.tryParse(r['text'] as String);
        if (o != null && o.tid.startsWith(m.group(1)!) && FileTransferService.safeName(o.name) == m.group(2)) tids.add(o.tid);
      }
      return tids.length == 1 ? tids.single : null;   // неоднозначно (одинаковые первые 8 знаков) — не угадываем
    } catch (_) {
      return null;
    }
  }

  /// Остатки выбора файлов (копии, которые делает системный выбор файлов в кеше) затираются: открытые копии не должны жить после
  /// сеанса, даже если отправка не состоялась или приложение убили.
  Future<void> _sweepPickerCache() async {
    final cache = await pp.getTemporaryDirectory();
    final d = Directory('${cache.path}/file_picker');
    if (!await d.exists()) return;
    final realCache = await cache.resolveSymbolicLinks();
    final realPicker = await d.resolveSymbolicLinks();
    if (!realPicker.startsWith('$realCache${Platform.pathSeparator}')) {
      protectionWarning = tr('Каталог временных копий выходит за пределы кеша; затирание отменено.');
      return;
    }
    var failed = 0;
    await for (final e in d.list(recursive: true, followLinks: false)) {
      if (e is File) {
        try { if (!await wipeFile(e.path) && await e.exists()) failed++; } catch (_) { failed++; }
      }
    }
    if (failed > 0) protectionWarning = trf('Не удалось затереть временных копий выбора файлов: {n}; попытка повторится при следующем входе.', {'n': failed});
  }

  /// Файлы можно слать только на своё устройство: у него есть ключ для E2E.
  Future<bool> canSendFiles(String peerId) async => await (_ws?.canEncryptTo(peerId) ?? Future.value(false));

  /// Отправить файл: шифрование на телефоне, куски на узел, ключ получателю в зашифрованном сообщении.
  @override
  int get transferCount => fileStates.length;

  /// Файлы, которые приложение готовит к отправке: в чате они видны сразу, до появления сообщения (см. `PrepTracker`).
  final PrepTracker _prep = PrepTracker();
  List<String> preparingFor(String peerId) => _prep.labels(peerId);

  String _prepLabel(String path, String? name) => (name != null && name.isNotEmpty) ? name : path.split('/').last;

  void _prepDone(String peerId, int id) {
    if (_prep.done(peerId, id)) notifyListeners();
  }

  @override
  Future<String?> sendFile(String peerId, String path, {String? name}) async {
    final label = _prepLabel(path, name);
    // Копией выбора владеют все операции, которые её читают: стирает последняя, кто отпустил (и отклонённый повтор, и очистка в
    // экране выбора не затирают источник идущей отправки).
    await _copies.acquire(path);
    try {
      // тот же источник, который уже готовится для этого собеседника, второй раз не принимается: раньше из-за задержки появления
      // сообщения файл отправляли повторно
      if (_prep.hasSource(peerId, path)) return trf('«{n}» уже готовится к отправке', {'n': label});
      final prep = _prep.start(peerId, label, path);
      notifyListeners();
      try {
        return await _sendFile(peerId, path, name: name, onPrepared: () => _prepDone(peerId, prep));
      } finally {
        _prepDone(peerId, prep);
      }
    } finally {
      // при любом исходе (отказ узла, ошибка, успех) открытая копия из выбора файла затирается — когда её отпустил последний читатель
      if (_copies.release(path) && !FileVault.isVaultPath(path)) await dropPickerCopy(path);
    }
  }

  Future<String?> _sendFile(String peerId, String path, {String? name, void Function()? onPrepared}) async {
    if (_api == null || _ws == null) return tr('Нет связи с узлом');
    if (appRevoked) return trf('Эта версия приложения ({v}) отозвана: обновите приложение до {m} или новее.', {'v': kAppVersion, 'm': requiredApp ?? ''});
    final svc = FileTransferService(_api!, vault: await VaultService.instance.files());
    late FileOffer offer;
    try {
      if (!await canSendFiles(peerId)) return tr('Файлы можно отправлять только на свои устройства, у этого собеседника нет ключа');
      offer = await svc.prepare(toPeerId: peerId, path: path, name: name);
    } on TransferException catch (e) {
      return e.message;
    } catch (e) {
      return trf('Не удалось начать отправку: {e}', {'e': e});
    }
    // Источник отправки — всегда зашифрованная копия в хранилище под номером передачи: открытая копия из выбора файла не остаётся,
    // а отправленный файл виден во вкладке «Файлы» и в чате.
    var source = path;
    if (!FileVault.isVaultPath(path)) {
      try {
        final vf = await vaultFiles();
        source = (await vf.importAs(path, offer.tid, name: name, mime: offer.mime)).path;
      } catch (e) {
        try { await _api!.deleteTransfer(offer.tid); } catch (_) {}
        return trf('Не удалось положить файл в защищённое хранилище: {e}', {'e': e});
      }
    }
    _outgoingPaths[offer.tid] = source;
    fileStates[offer.tid] = FileState.working(0);
    onPrepared?.call(); // строка «подготовка» уступает место сообщению с ходом отправки
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
      if (!sent) throw TransferException(tr('Файл залит, но предложение не отправлено: нет связи или ключа получателя'));
      fileStates.remove(offer.tid);
      final delivered = msg.copyWith(status: MessageStatus.delivered);
      _replaceMessage(peerId, msg.id, delivered);
      // saveMessage вставляет с ignore и уже существующую строку не обновит: статус меняем явно
      await StorageService.updateMessageStatus(msg.id, MessageStatus.delivered);
    } catch (e) {
      fileStates[offer.tid] = FileState.failed(e is TransferException ? e.message : '$e');
      // зашифрованная копия остаётся: сетевой сбой не должен уничтожать единственный экземпляр (например, голосового); она видна во
      // вкладке «Файлы», откуда её можно отправить заново или удалить. Открытая копия из выбора файла затирается как обычно.
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
      fileStates[o.tid] = FileState.failed(e is TransferException ? e.message : trf('Не удалось скачать: {e}', {'e': e}));
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
    replyId: m.replyId, replyText: m.replyText,
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
    final i = list == null ? -1 : list.indexWhere((m) => !m.outgoing && m.remoteId == cmid);
    if (list == null || i < 0) {
      // история этого чата не загружена (или сообщение вытеснено из последних 50): правка применяется прямо в базе, а не теряется
      unawaited(() async {
        final id = await StorageService.applyIncomingEdit(from, cmid, newText);
        if (id != null) {
          _editedIds.add(id);
          await StorageService.saveEditedIds(_editedIds);
        }
      }());
      return;
    }
    final local = list[i];
    list[i] = _withText(local, newText);
    _editedIds.add(local.id);
    unawaited(StorageService.updateMessageText(local.id, newText));
    unawaited(StorageService.saveEditedIds(_editedIds));
    notifyListeners();
  }

  // ── Сообщения, удалённые вместе с пользователем, и срок жизни сообщений ────────────────────────────────────────

  static const _forgottenCursorKey = 'forgotten_cursor_v1';
  static const _forgottenPeersKey = 'forgotten_peers_v1';
  /// Срок жизни сообщения (чаты и группы): 365 суток с отправки, без исключений. Файлы живут по своим срокам.
  static const messageLifetime = Duration(days: 365);

  /// Номер последней обработанной записи списка «убраны с узла» (сохраняется: после перезапуска список не перечитывается заново).
  int _forgottenCursor = 0;
  /// Собеседники, устройства которых убраны с узла: их сообщения из истории узла в базу заново не кладутся.
  final Set<String> _forgottenPeers = {};
  bool _forgottenLoaded = false;
  bool _forgottenBusy = false;
  bool _forgottenAgain = false;

  Future<void> _loadForgotten() async {
    if (_forgottenLoaded) return;
    _forgottenLoaded = true;
    try {
      _forgottenCursor = int.tryParse(await StorageService.secureRead(_forgottenCursorKey) ?? '') ?? 0;
      final raw = await StorageService.secureRead(_forgottenPeersKey);
      if (raw != null && raw.isNotEmpty) _forgottenPeers.addAll((jsonDecode(raw) as List).map((e) => '$e'));
    } catch (_) {}
  }

  /// Забрать с узла список убранных устройств после сохранённого курсора и превратить их сообщения в надгробия.
  /// Вызывается при каждом открытии сокета и по намёку 0x17; параллельные вызовы сливаются (один идёт, второй повторит проход).
  Future<void> syncForgotten() async {
    if (_forgottenBusy) { _forgottenAgain = true; return; }
    _forgottenBusy = true;
    final epoch = _lockEpoch;
    try {
      await _loadForgotten();
      do {
        _forgottenAgain = false;
        for (var page = 0; page < 50; page++) {
          final api = _api;
          if (api == null || epoch != _lockEpoch) return;
          final r = await api.getForgotten(_forgottenCursor);
          if (r == null || epoch != _lockEpoch) break;
          final items = r['items'] is List ? r['items'] as List : const [];
          var next = _forgottenCursor;
          for (final it in items) {
            if (it is! Map) continue;
            final n = it['n'], peer = it['peer'];
            if (n is! num) continue;
            if (peer is String && RegExp(r'^[0-9a-f]{64}$').hasMatch(peer)) await _forgetPeer(peer);
            if (n.toInt() > next) next = n.toInt();
          }
          final c = r['cursor'];
          if (c is num && c.toInt() > next) next = c.toInt();
          if (next > _forgottenCursor) {
            _forgottenCursor = next;
            await StorageService.secureWrite(_forgottenCursorKey, '$next');
          } else {
            break;
          }
          if (items.isEmpty) break;
        }
      } while (_forgottenAgain && epoch == _lockEpoch);
    } catch (_) {
      // курсор не сдвинут: следующий проход повторит (обработка устройства идемпотентна)
    } finally {
      _forgottenBusy = false;
    }
  }

  /// Устройство собеседника убрано с узла: его сообщения в личном чате — надгробия, цитаты их в моих ответах — тоже, смайлы стёрты.
  Future<void> _forgetPeer(String peer) async {
    final ids = await StorageService.tombstonePeer(peer);
    _forgottenPeers.add(peer);
    await StorageService.secureWrite(_forgottenPeersKey, jsonEncode(_forgottenPeers.toList()));
    await _loadRx();
    var rxChanged = false;
    for (final id in ids.incoming) {
      rxChanged = _rx.remove(id) != null || rxChanged;
      rxChanged = _rids.remove(id) != null || rxChanged;
    }
    for (final id in ids.outgoing) {
      final m = _rx[id];
      if (m != null && m.remove('peer') != null) {   // смайл этого собеседника под моим сообщением
        if (m.isEmpty) _rx.remove(id);
        rxChanged = true;
      }
    }
    if (rxChanged) _saveRx();
    final editedBefore = _editedIds.length;
    _editedIds.removeAll(ids.incoming);
    if (_editedIds.length != editedBefore) await StorageService.saveEditedIds(_editedIds);
    _clearUnread(peer);
    if (_chats.containsKey(peer)) {
      final epoch = _lockEpoch;
      final fresh = await _backfillRids(await StorageService.loadMessages(peer));
      if (epoch != _lockEpoch) return;
      _chats[peer] = fresh;
    }
    notifyListeners();
  }

  /// Удалить сообщения старше срока жизни (чаты и группы) вместе со смайлами и пометками «изменено». Файлы не трогаются.
  Future<void> pruneExpired() async {
    if (!StorageService.dbOpen) return;
    try {
      final cutoff = DateTime.now().subtract(messageLifetime).millisecondsSinceEpoch;
      final ids = (await StorageService.pruneOlderThan(cutoff)).toSet();
      if (ids.isEmpty) return;
      for (final l in _chats.values) { l.removeWhere((m) => ids.contains(m.id)); }
      await _loadRx();
      var rxChanged = false;
      for (final id in ids) {
        rxChanged = _rx.remove(id) != null || rxChanged;
        rxChanged = _rids.remove(id) != null || rxChanged;
      }
      if (rxChanged) _saveRx();
      final before = _editedIds.length;
      _editedIds.removeAll(ids);
      if (_editedIds.length != before) await StorageService.saveEditedIds(_editedIds);
      notifyListeners();
    } catch (_) {}
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
  String? get nodePeerId => _nodePeerId;

  /// Роль этого устройства на узле (`owner` | `user` | `invited` | `guest`) и право приглашать другие устройства — от узла.
  String? myRole;
  bool canInvite = false;
  /// Гость: ограниченная роль (нет групп, прокси, видео, приглашений); закрытое показывается с пояснением.
  bool get isGuest => myRole == 'guest';
  /// Запрос статуса пользователя у администратора: none / pending / accepted / rejected.
  String statusRequest = 'none';
  void _syncRoleFlags() { calls.audioOnly = isGuest; }

  Future<String?> requestUserStatus(String text) async {
    final err = await _api?.requestUserStatus(text);
    if (err == null) { statusRequest = 'pending'; notifyListeners(); }
    return err;
  }

  Future<void> refreshStatusRequest() async {
    final st = await _api?.getStatusRequest();
    if (st != null && st != statusRequest) { statusRequest = st; notifyListeners(); }
  }

  /// Приглашение для нового устройства: текст `YANDI-PAIR-1:…` (его же показывает QR), ссылка `yandi://invite?…`, 8-значный код, PIN из 4 цифр
  /// (его называют приглашённому другим каналом) и срок жизни. Живёт час и переживает закрытие приложения.
  Future<({String text, String link, String code, String pin, int ttlSecs})> createDeviceInvite() async {
    final node = activeNode;
    final api = _api;
    if (node == null || api == null) throw StateError(tr('Нет связи с узлом'));
    final inv = await api.createDeviceInvite();
    final expMs = DateTime.now().millisecondsSinceEpoch + inv.ttlSecs * 1000;
    final text = inviteText(host: node.host, port: node.port, code: inv.code, fingerprint: node.fingerprint, expiresAtSecs: expMs ~/ 1000);
    try {
      await InviteStore.saveIssued(node: node.id, text: text, code: inv.code, pin: inv.pin, expMs: expMs);
    } catch (_) {
      // запомнить на час не вышло (хранилище телефона): приглашение всё равно выдано и показано; после закрытия приложения оно не вернётся
    }
    return (text: text, link: inviteLink(text), code: inv.code, pin: inv.pin, ttlSecs: inv.ttlSecs);
  }

  /// Ещё живое выданное приглашение (после повторного открытия приложения) или null.
  Future<({String text, String link, String code, String pin, int ttlSecs})?> activeDeviceInvite() async {
    final m = await InviteStore.issued();
    // приглашение выдано на другом узле: здесь его не показываем (и отзывать на этом узле было бы неверно)
    if (m == null || m['node'] != activeNode?.id) return null;
    final left = ((m['exp'] as int) - DateTime.now().millisecondsSinceEpoch) ~/ 1000;
    if (left <= 0) return null;
    final text = '${m['text']}';
    return (text: text, link: inviteLink(text), code: '${m['code']}', pin: '${m['pin']}', ttlSecs: left);
  }

  Future<void> revokeDeviceInvites() async {
    // сначала узел: при ошибке код продолжает действовать, и запись у нас остаётся, чтобы отзыв можно было повторить
    await _api?.revokeDeviceInvites();
    final m = await InviteStore.issued();
    if (m != null && m['node'] == activeNode?.id) await InviteStore.clearIssued();
  }
  final Set<String> _avatarAnnounced = {};
  DateTime _lastAvatarSync = DateTime.now();

  /// Картинка контакта. У «компьютера» (моего же узла) та же картинка, что у меня: профиль общий на все мои устройства.
  Uint8List? avatarBytes(String peerId) => peerId == _nodePeerId ? null : (avatars?.bytesForPeer(peerId) ?? _directoryAvatars[peerId]);
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
      _profileChanged();
      return null;
    } catch (e) {
      return trf('Не удалось подготовить картинку: {e}', {'e': e.toString().split('\n').first});
    }
  }

  Future<String?> setMyAvatarEmoji(String emoji) async {
    try {
      await avatars!.setMine(await AvatarProcessor.fromEmoji(emoji));
      _avatarAnnounced.clear();
      _announceAvatar();
      _profileChanged();
      return null;
    } catch (e) {
      return trf('Не удалось подготовить картинку: {e}', {'e': e.toString().split('\n').first});
    }
  }

  /// Своя картинка контакту (видна только мне).
  Future<String?> setContactPicture(String peerId, Uint8List? source, {String? emoji}) async {
    try {
      final png = emoji != null ? await AvatarProcessor.fromEmoji(emoji) : await AvatarProcessor.fromImageBytes(source!);
      await avatars!.setLocal(peerId, png);
      notifyListeners();
      return null;
    } catch (e) {
      return trf('Не удалось подготовить картинку: {e}', {'e': e.toString().split('\n').first});
    }
  }

  Future<void> clearContactPicture(String peerId) async {
    await avatars?.clearLocal(peerId);
    notifyListeners();
  }

  bool hasContactPicture(String peerId) => avatars?.hasLocal(peerId) ?? false;

  Future<void> clearMyAvatar() async {
    await avatars?.clearMine();
    _avatarAnnounced.clear();
    _announceAvatar(force: true);   // сообщить контактам, что картинки больше нет
    _profileChanged();
  }

  // ── единый профиль: ник и картинка хранятся на узле и общие для ПК и всех телефонов ──
  String myNick = '';
  int _profileSeenMs = 0;     // версия профиля узла, которую мы видели в последний раз
  bool _profileDirty = false; // есть свои правки, ещё не доставленные на узел

  Future<void> _loadProfileMeta() async {
    try {
      final raw = await StorageService.secureRead('profile_v1');
      if (raw == null || raw.isEmpty) return;
      final m = jsonDecode(raw) as Map<String, dynamic>;
      myNick = (m['nick'] as String?) ?? '';
      _profileSeenMs = (m['seen'] as num?)?.toInt() ?? 0;
      _profileDirty = m['dirty'] == true;
    } catch (_) {}
  }

  Future<void> _saveProfileMeta() =>
      StorageService.secureWrite('profile_v1', jsonEncode({'nick': myNick, 'seen': _profileSeenMs, 'dirty': _profileDirty}));

  int _profileRev = 0;   // номер правки профиля: «доставлено» снимается только для той правки, что действительно ушла

  void _profileChanged() {
    _profileRev++;
    _profileDirty = true;
    unawaited(_saveProfileMeta());
    unawaited(syncProfile());
    unawaited(groups?.refreshCards(myNick, avatars?.myBytes) ?? Future<void>.value());   // там, где раскрыт ник, обновить визитку
  }

  /// Задать свой ник (виден контактам и в группах). null — готово, иначе причина.
  Future<String?> setMyNick(String nick) async {
    final n = nick.trim();
    if (n.isEmpty) return tr('Ник не может быть пустым');
    if (n.runes.length > 40) return tr('Ник не длиннее 40 знаков');
    myNick = n;
    notifyListeners();
    _profileChanged();
    return null;
  }

  bool _profileSyncing = false;

  /// Сверка с узлом: свои несохранённые правки уходят на узел (последняя правка побеждает), иначе подтягивается то, что поменяли
  /// на другом устройстве.
  bool _deviceProfilePublished = false;

  /// Профиль этого устройства для списка «Пользователи» узла (ник и картинка, как человек их задал).
  Future<void> _publishDeviceProfile() async {
    final api = _api;
    if (api == null) return;
    final bytes = avatars?.myBytes;
    final ok = await api.putDeviceProfile(nick: myNick, avatarB64: bytes == null ? '' : base64.encode(bytes));
    if (ok) _deviceProfilePublished = true;
  }

  /// Жалоба узлу. [attachRecent] — приложить последние входящие сообщения этого человека (доказательства выбирает заявитель).
  Future<String?> complain({required String kind, required String targetId, required String reason, required String text, bool attachRecent = false}) {
    final api = _api;
    if (api == null) return Future.value(tr('Нет связи с узлом'));
    final evidence = <String>[];
    if (attachRecent && kind == 'user') {
      final inc = messagesFor(targetId).where((m) => !m.outgoing).toList();
      for (final m in inc.where((m) => !Tombstone.isMarker(m.text)).toList().reversed.take(5).toList().reversed) {
        evidence.add(m.text);
      }
    }
    return api.submitComplaint(kind: kind, targetId: targetId, reason: reason, text: text, evidence: evidence);
  }

  /// Логотипы групп (только в памяти): группа → (хэш, PNG).
  final Map<String, (String, Uint8List)> _groupLogos = {};
  final Set<String> _logoFetching = {};

  Uint8List? groupLogo(String gid, String hash) {
    final v = _groupLogos[gid];
    return (v != null && v.$1 == hash) ? v.$2 : null;
  }

  Future<void> ensureGroupLogo(String gid, String hash) async {
    if (hash.isEmpty || groupLogo(gid, hash) != null || !_logoFetching.add(gid)) return;
    try {
      final png = await _api?.getGroupLogo(gid);
      if (png != null) {
        _groupLogos[gid] = (hash, png);
        notifyListeners();
      }
    } finally {
      _logoFetching.remove(gid);
    }
  }

  /// Пользователи узла со своими никами и картинками (только в памяти, на диск не пишутся).
  List<Map<String, dynamic>> directory = const [];
  /// Последний удачный ответ узла со списком людей (для не-владельца): при сбое сети список не пустеет.
  List<Map<String, dynamic>> _lastUsers = const [];
  final Map<String, Uint8List> _directoryAvatars = {};
  final Map<String, String> _directoryHashes = {};

  Future<void> refreshDirectory() async {
    final api = _api;
    if (api == null) return;
    final gen = _dataGen;
    final users = await api.getUsers();
    if (gen != _dataGen) { unawaited(refreshDirectory()); return; }   // поколение сменилось: начать заново уже у нового узла
    if (users == null || users.isEmpty) return;
    directory = users;
    notifyListeners();
    for (final u in users) {
      final id = u['peer_id'] as String? ?? '';
      final hash = u['avatar_hash'] as String? ?? '';
      if (id.isEmpty) continue;
      if (hash.isEmpty) {
        if (_directoryAvatars.remove(id) != null) notifyListeners();
        _directoryHashes.remove(id);
        continue;
      }
      if (_directoryHashes[id] == hash && _directoryAvatars.containsKey(id)) continue;
      final png = await api.getUserAvatar(id);
      if (gen != _dataGen) { unawaited(refreshDirectory()); return; }   // узел сменился, пока шла картинка: начать заново
      if (png != null && AvatarMsg.hashOf(png) == hash) {
        _directoryAvatars[id] = png;
        _directoryHashes[id] = hash;
        notifyListeners();
      }
    }
  }

  /// Профиль теперь свой у каждого устройства: общий с ПК профиль не подтягивается и не перезаписывается. Здесь только публикация
  /// своих ника и картинки на узел (для списка «Пользователи»), когда человек их менял или это первый вход.
  Future<void> syncProfile() async {
    final api = _api;
    if (api == null || _profileSyncing) return;
    _profileSyncing = true;
    try {
      // за время отправки человек мог сменить ник ещё раз: тогда отправляем снова, а не считаем новое значение доставленным
      for (var attempt = 0; attempt < 4 && (_profileDirty || !_deviceProfilePublished); attempt++) {
        final rev = _profileRev;
        await _publishDeviceProfile();
        if (!_deviceProfilePublished) break;
        if (rev == _profileRev) {
          _profileDirty = false;
          await _saveProfileMeta();
          break;
        }
      }
    } catch (_) {
      // узел недоступен — повторим в следующий проход
    } finally {
      _profileSyncing = false;
    }
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

  /// «Игнорировать» запрос знакомства: непрочитанное снимается, человек не в контактах и не заблокирован.
  void dismissIntro(String peerId) => _clearUnread(peerId);

  /// Не более стольких сообщений от человека, которого вы ещё не приняли в контакты: дальше молча не принимаются.
  static const int introLimit = 3;

  /// Знакомства: люди не из контактов, написавшие вам и ещё не принятые и не заблокированные.
  List<Contact> get introRequests => [
        for (final c in contacts)
          if (!c.saved && !isBlocked(c.peerId) && unreadFor(c.peerId) > 0 && c.peerId != _nodePeerId) c,
      ];

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
  (String, String?, String?, String?)? _classifyIncoming(String from, String rawText) {
    final av = AvatarMsg.tryParse(rawText);
    if (av != null) { unawaited(_onAvatarMsg(from, av)); return null; }
    if (AvatarMsg.isAvatar(rawText)) return null;   // повреждённый служебный конверт аватарки — не показываем как текст
    final rcpt = MsgChannel.parseReceipt(rawText);
    if (rcpt != null) { _applyReceipt(from, rcpt.$1, rcpt.$2); return null; }
    final edit = MsgChannel.parseEdit(rawText);
    if (edit != null) { _applyEdit(from, edit.$1, edit.$2); return null; }
    if (rawText.startsWith(MsgChannel.rxMarker)) {
      final rx = MsgChannel.parseReaction(rawText);
      if (rx != null) _applyReaction(from, rx.$1, rx.$2);
      return null;   // и неверный конверт реакции не показывается как текст
    }
    if (MsgChannel.isCapPing(rawText)) { _receiptCapable.add(from); return null; }
    final env = MsgChannel.parseMessageFull(rawText);
    if (env != null) { _receiptCapable.add(from); return (env.$2, env.$1, env.$3, env.$4); } // (cmid, text, rid, rtext) -> (text, cmid, rid, rtext)
    return (rawText, null, null, null);
  }

  void _applyReceipt(String from, String kind, List<String> ids) {
    final target = kind == 'read' ? MessageStatus.read : MessageStatus.delivered;
    // и в базу — независимо от того, загружен ли чат: иначе квитанция «подтверждена», а статус остался старым
    unawaited(StorageService.raiseOutgoingStatus(from, ids, target));
    final list = _chats[from];
    if (list == null) return;
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

  // ── реакции личного чата: простые смайлы, у каждой стороны не больше одной на сообщение ──

  /// локальный номер сообщения → {'me': смайл, 'peer': смайл}; хранится в защищённом хранилище
  final Map<String, Map<String, String>> _rx = {};
  /// локальный номер входящего сообщения → его номер у отправителя (чтобы ссылаться на него после перезапуска)
  final Map<String, String> _rids = {};
  bool _rxLoaded = false;

  Future<void> _loadRx() async {
    if (_rxLoaded) return;
    _rxLoaded = true;
    try {
      final raw = await StorageService.secureRead('reactions_v1');
      if (raw == null) return;
      final j = jsonDecode(raw) as Map<String, dynamic>;
      ((j['rx'] as Map?) ?? {}).forEach((k, v) {
        if (v is Map) _rx['$k'] = {for (final e in v.entries) if (e.value is String) '${e.key}': e.value as String};
      });
      ((j['rid'] as Map?) ?? {}).forEach((k, v) { if (v is String) _rids['$k'] = v; });
    } catch (_) {}
  }

  void _saveRx() {
    if (_rids.length > 3000) {
      for (final k in _rids.keys.take(_rids.length - 3000).toList()) { _rids.remove(k); }
    }
    unawaited(StorageService.secureWrite('reactions_v1', jsonEncode({'rx': _rx, 'rid': _rids})));
  }

  /// Смайлы под сообщением: (мой, собеседника) — пустая строка, если нет.
  (String, String) reactionsOf(String msgId) => (_rx[msgId]?['me'] ?? '', _rx[msgId]?['peer'] ?? '');

  /// Номер сообщения так, как его знает собеседник (null — неизвестен: сообщение пришло в прошлом запуске без конверта).
  String? _remoteKey(ChatMessage m) => m.outgoing ? m.id : (m.remoteId ?? _rids[m.id]);

  /// Поставить смайл на сообщение (тот же смайл ещё раз — снять). null — готово, иначе причина.
  Future<String?> react(ChatMessage m, String emoji) async {
    if (!MsgChannel.reactionSet.contains(emoji)) return tr('Такого смайла нет');
    final key = _remoteKey(m);
    if (key == null) return tr('Это сообщение пришло без номера — на него нельзя отреагировать');
    if (!_receiptCapable.contains(m.peerId)) return tr('Собеседнику нужно обновить приложение, чтобы видеть смайлы');
    await _loadRx();
    final cur = _rx[m.id]?['me'] ?? '';
    final next = cur == emoji ? '' : emoji;
    (_rx[m.id] ??= {})['me'] = next;
    if (next.isEmpty && (_rx[m.id]?['peer'] ?? '').isEmpty) _rx.remove(m.id);
    _saveRx();
    notifyListeners();
    unawaited(_ws?.sendMessage(m.peerId, MsgChannel.reaction(key, next)) ?? Future<bool>.value(false));
    return null;
  }

  void _applyReaction(String from, String cmid, String emoji) {
    unawaited(() async {
      await _loadRx();
      final list = _chats[from] ?? const <ChatMessage>[];
      String? local;
      // реакция на моё сообщение — номер мой; на его — номер у него (в памяти или в сохранённом соответствии)
      for (final m in list) {
        if (m.outgoing ? m.id == cmid : (m.remoteId == cmid || _rids[m.id] == cmid)) { local = m.id; break; }
      }
      local ??= _rids.entries.where((e) => e.value == cmid).map((e) => e.key).firstOrNull;
      if (local == null) return;
      if (emoji.isEmpty) {
        _rx[local]?.remove('peer');
        if ((_rx[local] ?? const {}).isEmpty) _rx.remove(local);
      } else {
        (_rx[local] ??= {})['peer'] = emoji;
      }
      _saveRx();
      notifyListeners();
    }());
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

  /// Ответ-цитата на сообщение: id в понимании собеседника — наш номер (он его знает как cmid) для своих, его cmid (remoteId) для чужих.
  static String? replyIdFor(ChatMessage m) => m.outgoing ? m.id : m.remoteId;

  /// Найти в чате сообщение, на которое ответили: по id, затем по выдержке текста (узел называет сообщения телефона своими номерами).
  ChatMessage? findReplyTarget(String peerId, ChatMessage m) {
    final text = m.replyText;
    if (text == null) return null;
    final list = _chats[peerId] ?? const <ChatMessage>[];
    final id = m.replyId ?? '';
    if (id.isNotEmpty) {
      for (final x in list) {
        if (x.id == id && x.outgoing) return x;
        if (!x.outgoing && x.remoteId == id) return x;
      }
    }
    for (final x in list) {
      if (x.id != m.id && x.text.isNotEmpty && x.text.startsWith(text)) return x;
    }
    return null;
  }

  /// Сколько сообщений гость ещё может отправить за скользящие сутки (docs/ROADMAP_R9.md §2); не гость — без лимита (-1).
  /// Узел дополнительно держит свой потолок кадров (`guest::MAX_MSG_FRAMES_PER_DAY`).
  static const guestMessagesPerDay = 10;
  static const _guestSendsKey = 'guest_sends_v1';
  /// Времена отправок гостя (мс) за скользящие сутки: отдельное маленькое хранилище, не история чатов (очистка чата не обнуляет лимит).
  List<int> _guestSends = [];
  bool _guestSendsLoaded = false;

  Future<void> _loadGuestSends() async {
    if (_guestSendsLoaded) return;
    _guestSendsLoaded = true;
    try {
      final raw = await StorageService.secureRead(_guestSendsKey);
      final l = raw == null ? null : jsonDecode(raw);
      if (l is List) _guestSends = [for (final x in l) if (x is int) x];
    } catch (_) {}
    _pruneGuestSends();
  }

  void _pruneGuestSends() {
    final now = DateTime.now().millisecondsSinceEpoch;
    // запись «из будущего» (часы переведены назад) тоже считается за последние сутки, но не дольше суток от сейчас
    _guestSends = [for (final t in _guestSends) if (now - t < 86400000 && t <= now + 86400000) t];
    if (_guestSends.length > guestMessagesPerDay * 4) _guestSends = _guestSends.sublist(_guestSends.length - guestMessagesPerDay * 4);
  }

  int get guestMessagesLeft {
    if (!isGuest) return -1;
    _pruneGuestSends();
    return (guestMessagesPerDay - _guestSends.length).clamp(0, guestMessagesPerDay);
  }

  /// Попытка не дошла до сокета (нет связи или ключа): в лимит она не идёт, на узле её кадр не посчитан.
  Future<void> _guestSendRefund() async {
    if (!isGuest || _guestSends.isEmpty) return;
    _guestSends.removeLast();
    try {
      await StorageService.secureWrite(_guestSendsKey, jsonEncode(_guestSends));
    } catch (_) {}
  }

  /// Учесть попытку отправки гостя (в том числе повтор): false — лимит исчерпан, отправлять нельзя. Не гость — всегда true.
  /// Запись делается до отправки и сразу сохраняется, чтобы перезапуск приложения лимит не сбрасывал.
  Future<bool> _guestSendAllowed() async {
    if (!isGuest) return true;
    await _loadGuestSends();
    if (guestMessagesLeft == 0) return false;
    _guestSends.add(DateTime.now().millisecondsSinceEpoch);
    try {
      await StorageService.secureWrite(_guestSendsKey, jsonEncode(_guestSends));
    } catch (_) {}
    return true;
  }

  Future<void> sendMessage(String peerId, String text, {ChatMessage? replyTo}) async {
    if (isBlocked(peerId)) return;   // двусторонний блок: заблокированному не отправляем
    if (appRevoked) return;   // сборка отозвана: до обновления ничего не отправляем
    if (!await _guestSendAllowed()) return; // гость исчерпал сообщения на сутки: экран чата показывает причину
    final id = ChatMessage.generateId();
    // Всегда конверт с id: получатель по нему отвечает «доставлено/прочитано». Раньше обёртка зависела от того, успел ли собеседник ответить
    // на пинг поддержки в этом сеансе: после запуска приложения это знание пусто, и всё отправленное в это окно уходило без номера — без галочек.
    // Шифровать мы можем только устройствам со связкой ключей E4, то есть новым сборкам, а они квитанции умеют.
    final replyText = replyTo == null ? null : _excerpt(FileOffer.tryParse(replyTo.text)?.name ?? replyTo.text);
    final replyId = replyTo == null ? null : replyIdFor(replyTo);
    final payload = MsgChannel.wrapMessage(id, text, replyId: replyId, replyText: replyText);
    final sent = await (_ws?.sendMessage(peerId, payload) ?? Future<bool>.value(false));
    if (!sent) await _guestSendRefund();

    final msg = ChatMessage(
      id:        id,
      peerId:    peerId,
      outgoing:  true,
      text:      text,
      timestamp: DateTime.now(),
      status:    sent ? MessageStatus.pending : MessageStatus.failed,
      replyId:   replyId,
      replyText: replyText,
    );
    _chats.putIfAbsent(peerId, () => []).add(msg);
    await StorageService.saveMessage(msg);
    notifyListeners();
  }

  static String _excerpt(String t) {
    final s = t.replaceAll(RegExp(r'\s+'), ' ').trim();
    return s.runes.length <= 200 ? s : String.fromCharCodes(s.runes.take(200));
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
    try {
      await _handleIncomingChat(e);
    } catch (_) {
      e.abort?.call();   // например, не записалась база: повтор этого письма не должен считаться «уже в работе»
    }
  }

  Future<void> _handleIncomingChat(IncomingChatEvent e) async {
    if (isBlocked(e.fromPeerId)) { await e.commit?.call(); return; }   // чёрный список: не показываем и не подтверждаем
    // незнакомый человек: до принятия в контакты принимаются только первые несколько сообщений (защита от спама)
    if (!isSaved(e.fromPeerId) && e.fromPeerId != _nodePeerId &&
        (_chats[e.fromPeerId]?.where((m) => !m.outgoing).length ?? 0) >= introLimit) {
      await e.commit?.call();
      return;
    }
    final res = _classifyIncoming(e.fromPeerId, e.text);
    if (res == null) { await e.commit?.call(); notifyListeners(); return; } // служебное (квитанция/пинг) — не показываем
    final showText = res.$1;
    final remoteId = res.$2;
    final msg = ChatMessage(
      id:        ChatMessage.incomingId(e.fromPeerId, e.timestamp, showText),
      peerId:    e.fromPeerId,
      outgoing:  false,
      text:      showText,
      timestamp: e.timestamp,
      status:    MessageStatus.delivered,
      remoteId:  remoteId,
      replyId:   res.$3,
      replyText: res.$4,
    );
    // запись и проверка «не было ли уже» — одна операция базы; в память и в уведомления попадает только тот, кто записал
    final fresh = await StorageService.insertIncomingIfNew(msg);
    if (!fresh) {
      // повторная отправка того же сообщения (после обрыва) — второй раз не показываем, но подтверждаем, чтобы отправитель перестал повторять
      await e.commit?.call();
      if (remoteId != null) _sendDeliveredReceipt(e.fromPeerId, remoteId);
      return;
    }
    _chats.putIfAbsent(e.fromPeerId, () => []).add(msg);
    await e.commit?.call();   // номер письма занимается только после записи в БД
    if (remoteId != null) { _rids[msg.id] = remoteId; _saveRx(); }
    if (activeChatPeerId != e.fromPeerId) _bumpUnread(e.fromPeerId);
    if (remoteId != null) {
      _sendDeliveredReceipt(e.fromPeerId, remoteId);
      // чат открыт — сразу «прочитано»
      if (activeChatPeerId == e.fromPeerId && !_readAcked.contains(remoteId)) {
        _readAcked.add(remoteId);
        unawaited(_ws?.sendMessage(e.fromPeerId, MsgChannel.receipt('read', [remoteId])) ?? Future<bool>.value(false));
      }
    }

    // An open chat changes read receipts, not the user's sound preference.
    if (WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed) {
      unawaited(playAppSound('messages'));
    }
    if (activeChatPeerId != e.fromPeerId) {
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
    // тип файла, который в сети не передаётся (старый или чужой клиент), не принимается: ни предложения, ни уведомления
    if (attachmentProblem(e.fileName) != null) return;
    pendingFileOffers.add(e);
    unawaited(playAppSound('messages'));
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
    _lockEpoch++;
    _presenceTimer?.cancel();
    _updateTimer?.cancel();
    _pruneTimer?.cancel();
    appUpdate = null;
    _retryTimer?.cancel();
    _ws?.dispose();
    _ws = null;
    _api?.dispose();
    _api = null;
    nodeOnline = false;
    nodeError = null;
    identity = null;
    groups = null;
    avatars?.wipeMemory();
    avatars = null;
    _vaultFiles = null;
    VaultBytesCache.clear();
    VideoFrames.clear();
    // картинки, уже отданные Flutter для показа (кеш изображений), тоже убираем
    PaintingBinding.instance.imageCache.clear();
    PaintingBinding.instance.imageCache.clearLiveImages();
    // уведомления, показанные до блокировки, снимаются
    unawaited(NotificationService.cancelAll());
    contacts = [];
    nodeManager.clear();
    _chats.clear();
    _unread.clear();
    _blocked.clear();
    _editedIds.clear();
    _readAcked.clear();
    pendingFileOffers.clear();
    activeChatPeerId = null;
    // всё, что приложение держит о людях и группах в памяти: справочник, картинки, логотипы, ник, скрытые, квитанции
    directory = const [];
    _lastUsers = const [];
    _dataGen++;
    for (final b in _directoryAvatars.values) { b.fillRange(0, b.length, 0); }
    _directoryAvatars.clear();
    _directoryHashes.clear();
    for (final (_, png) in _groupLogos.values) { png.fillRange(0, png.length, 0); }
    _groupLogos.clear();
    myNick = '';
    hiddenUsers.clear();
    hiddenGroups.clear();
    _rx.clear();
    _rids.clear();
    _forgottenPeers.clear();
    _forgottenCursor = 0;
    _forgottenLoaded = false;
    notifyListeners();
  }

  /// «Удалить всё»: службы останавливаются, память очищается, затем стираются ключи, база, файлы и секреты.
  Future<String?> eraseEverything() async {
    try { await chatBg.stop(); } catch (_) {}
    // VPN держит токен устройства в памяти службы: до стирания ключей он останавливается, иначе прежняя авторизация живёт вне стёртой среды
    try { await vpn.stop(); } catch (_) {}
    vpnRunning = false;
    try { await lockDown(); } catch (_) {}
    return VaultService.instance.eraseEverything();
  }

  @override
  void dispose() {
    _calls?.dispose();
    _ws?.dispose();
    _api?.dispose();
    _player.dispose();
    super.dispose();
  }
}

void unawaited(async_lib.Future<void> f) { async_lib.unawaited(f); }
