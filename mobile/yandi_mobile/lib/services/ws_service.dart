import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/foundation.dart' show visibleForTesting;
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:web_socket_channel/io.dart';
import '../models/trusted_node.dart';
import '../crypto/identity.dart';
import '../crypto/e2e_crypto.dart';
import '../crypto/peer_trust.dart';
import 'storage_service.dart';

const int ftPing       = 0x01;
const int ftPong       = 0x02;
const int ftChatMsg    = 0x10;
const int ftMsgStatus  = 0x11;
const int ftPeerStatus = 0x12;
const int ftSendMsg    = 0x30;
// сигналы звонков: только вживую, узел их не хранит (зашифрованы телефоном для телефона)
const int ftLiveMsg     = 0x13;
const int ftPeerOffline = 0x14;
const int ftSendLive    = 0x31;
// намёк «в группе что-то изменилось» (без содержимого): [0x15][16 номер группы][1 что][8 seq LE]
const int ftGroupHint   = 0x15;
const int ftProfile     = 0x16;

class GroupHintEvent {
  final String gid;
  final int what;   // 1 запись (новая или стёртая), 2 ключи, 3 состав/настройки, 4 группа удалена
  final int seq;
  const GroupHintEvent(this.gid, this.what, this.seq);
}

class IncomingChatEvent {
  final String   fromPeerId;
  final DateTime timestamp;
  final String   text;
  /// Занять номер письма в журнале повторов: вызывается ПОСЛЕ сохранения сообщения (до этого повтор не считается «виденным»).
  final Future<bool> Function()? commit;
  const IncomingChatEvent({required this.fromPeerId, required this.timestamp, required this.text, this.commit});
}

/// Живой сигнал от другого телефона (звонок): расшифрованный текст.
class LiveSignalEvent {
  final String   fromPeerId;
  final DateTime timestamp;
  final String   text;
  const LiveSignalEvent({required this.fromPeerId, required this.timestamp, required this.text});
}

class PeerStatusEvent {
  final String peerId;
  final bool   online;
  const PeerStatusEvent(this.peerId, this.online);
}

class FileOfferEvent {
  final String fromPeerId;
  final String transferId;
  final String fileName;
  final int    fileSize;
  const FileOfferEvent({
    required this.fromPeerId,
    required this.transferId,
    required this.fileName,
    required this.fileSize,
  });
}

/// WebSocket сервис — бинарный протокол с E2E шифрованием.
///
/// Шифрование прозрачно для ноды: она видит только зашифрованный blob.
/// Расшифровка происходит на мобилке приватным X25519 ключом Identity.
class WsService {
  final Identity _identity;

  /// Вызывается когда переподключение к текущей ноде не удалось
  /// (задержка достигла максимума). AppState использует это для failover.
  final Future<void> Function()? onPermanentLoss;

  TrustedNode?        _node;
  WebSocketChannel?   _channel;
  Timer?              _pingTimer;
  Timer?              _reconnectTimer;
  int                 _reconnectDelay = 5;
  int                 _reconnectAttempts = 0;
  bool                _lossReported = false;
  /// 5+10+20+40+60+60 с ≈ 3 минуты без связи
  static const        _lossAfterAttempts = 6;
  bool                _screenOn       = true;

  // Адаптивный интервал: 30с экран вкл, 90с экран выкл
  Duration get _pingInterval => _screenOn
      ? const Duration(seconds: 30)
      : const Duration(seconds: 90);

  final _chatCtrl      = StreamController<IncomingChatEvent>.broadcast();
  final _statusCtrl    = StreamController<PeerStatusEvent>.broadcast();
  final _fileOfferCtrl = StreamController<FileOfferEvent>.broadcast();
  final _liveCtrl      = StreamController<LiveSignalEvent>.broadcast();
  final _offlineCtrl   = StreamController<String>.broadcast();
  final _groupCtrl     = StreamController<GroupHintEvent>.broadcast();
  final _profileCtrl   = StreamController<void>.broadcast();

  Stream<IncomingChatEvent> get chatStream      => _chatCtrl.stream;
  Stream<PeerStatusEvent>   get statusStream    => _statusCtrl.stream;
  Stream<FileOfferEvent>    get fileOfferStream => _fileOfferCtrl.stream;
  Stream<LiveSignalEvent>   get liveStream      => _liveCtrl.stream;
  /// Номер телефона, которому сигнал не доставлен: его нет на связи.
  Stream<String>            get offlineStream   => _offlineCtrl.stream;
  Stream<GroupHintEvent>    get groupStream     => _groupCtrl.stream;
  /// Узел сообщил: профиль владельца (ник или картинка) изменился — подтянуть его.
  Stream<void>              get profileStream   => _profileCtrl.stream;
  bool get connected => _channel != null;

  /// Вызывается, когда сокет к узлу реально открыт (в том числе после переподключения): самое время обновить статусы.
  final void Function()? onOpen;

  WsService(this._identity, {this.onPermanentLoss, this.onOpen,
    Future<PeerKeyBundle?> Function(String)? getPeerBundle,
    Future<String?> Function()? ownPeerId})
      : _getPeerBundle = getPeerBundle, _ownPeerId = ownPeerId;

  final Future<PeerKeyBundle?> Function(String)? _getPeerBundle;
  final Future<String?> Function()? _ownPeerId;

  // ── Управление подключением ────────────────────────────────────────────────

  void connect(TrustedNode node) {
    _node = node;
    _reconnectDelay = 5;
    _tryConnect();
  }

  /// Переключиться на другую ноду (failover).
  void switchNode(TrustedNode node) {
    _pingTimer?.cancel();
    _reconnectTimer?.cancel();
    _channel?.sink.close();
    _channel = null;
    _node    = node;
    _reconnectDelay = 5;
    _tryConnect();
  }

  void _tryConnect() {
    final node = _node;
    if (node == null || node.token == null) return;

    // #4 аудита: токен в заголовке Authorization, а не в URL сокета (URL может попасть в логи/дампы)
    final wsUrl = Uri.parse('${node.wsUrl}/mobile/ws');
    try {
      final httpClient = _buildPinnedClient(node.fingerprint);
      _channel = IOWebSocketChannel.connect(
        wsUrl,
        customClient: httpClient,
        headers: {'Authorization': 'Bearer ${node.token}'},
      );
      final ch = _channel!;
      ch.ready.then((_) {
        if (!identical(_channel, ch)) return;
        // задержку и счёт неудач сбрасываем только когда сокет действительно открылся, а не сразу после попытки
        _reconnectDelay = 5;
        _reconnectAttempts = 0;
        _lossReported = false;
        onOpen?.call();
      }).catchError((_) {});
      // обратные вызовы старого сокета (после переключения узла) не трогают новое соединение
      ch.stream.listen(
        _onData,
        onError: (_) { if (identical(_channel, ch)) _scheduleReconnect(); },
        onDone:  ()  { if (identical(_channel, ch)) _scheduleReconnect(); },
      );
      _startPingTimer();
    } catch (_) {
      _scheduleReconnect();
    }
  }

  void _scheduleReconnect() {
    _pingTimer?.cancel();
    _channel = null;
    _reconnectTimer?.cancel();
    _reconnectAttempts++;
    // узел не отвечает уже несколько минут (задержка дошла до предела): один раз просим приложение перейти на другой доверенный узел
    if (_reconnectAttempts >= _lossAfterAttempts && !_lossReported) {
      _lossReported = true;
      unawaited(onPermanentLoss?.call());
    }
    _reconnectTimer = Timer(Duration(seconds: _reconnectDelay), () {
      _reconnectDelay = (_reconnectDelay * 2).clamp(5, 60);
      _tryConnect();
    });
  }

  // ── Адаптивный ping ────────────────────────────────────────────────────────

  void _startPingTimer() {
    _pingTimer?.cancel();
    _pingTimer = Timer.periodic(_pingInterval, (_) {
      _sendRaw(Uint8List.fromList([ftPing]));
    });
  }

  /// Вызвать когда экран включается/выключается.
  void onScreenStateChanged(bool isOn) {
    if (_screenOn == isOn) return;
    _screenOn = isOn;
    if (connected) _startPingTimer(); // перезапустить с новым интервалом
  }

  // ── Входящие данные ────────────────────────────────────────────────────────

  static const _maxLiveInFlight = 8;
  int _liveInFlight = 0;

  /// Только для тестов: подать кадр сокета так, как будто он пришёл с узла.
  @visibleForTesting
  Future<void> debugFeed(List<int> raw) async => _onData(raw);

  @visibleForTesting
  int get debugLiveInFlight => _liveInFlight;

  void _onData(dynamic raw) async {
    if (raw is! List<int>) return;
    final data = Uint8List.fromList(raw);
    if (data.isEmpty) return;

    switch (data[0]) {
      case ftPing:
        _sendRaw(Uint8List.fromList([ftPong]));

      case ftChatMsg:
        await _handleChatMsg(data);

      case ftPeerStatus:
        _handlePeerStatus(data);

      case ftLiveMsg:
        // живые сигналы эфемерны, а проверка каждого стоит запроса связки ключей и подписи: больше _maxLiveInFlight сразу не берём
        if (_liveInFlight >= _maxLiveInFlight) return;
        _liveInFlight++;
        try {
          await _handleLiveMsg(data);
        } finally {
          _liveInFlight--;
        }

      case ftGroupHint:
        if (data.length >= 26) {
          final seq = ByteData.sublistView(data, 18, 26).getUint64(0, Endian.little);
          _groupCtrl.add(GroupHintEvent(_bytesToHex(data.sublist(1, 17)), data[17], seq));
        }

      case ftProfile:
        _profileCtrl.add(null);

      case ftPeerOffline:
        if (data.length >= 33) _offlineCtrl.add(_bytesToHex(data.sublist(1, 33)));
    }
  }

  /// Письма, у которых номер уже проверен, но ещё не занят (идёт сохранение): параллельный повтор не обрабатывается дважды.
  final Set<String> _replayInFlight = {};

  Future<void> _handleChatMsg(Uint8List data) async {
    // [1B type][32B from][8B ts_ms_LE][4B payload_len_LE][payload]
    if (data.length < 45) return;
    final from    = _bytesToHex(data.sublist(1, 33));
    final tsMs    = ByteData.sublistView(data, 33, 41).getInt64(0, Endian.little);
    final pLen    = ByteData.sublistView(data, 41, 45).getUint32(0, Endian.little);
    if (data.length < 45 + pLen) return;
    final payload = data.sublist(45, 45 + pLen);
    if (tsMs < 0 || tsMs > _maxDateMs) return;   // кривое время в заголовке: отбрасываем до расшифровки (журнал повторов не тратим)

    // номер письма занимается после сохранения (см. IncomingChatEvent.commit): сбой между расшифровкой и записью не теряет сообщение
    String? pendingId;
    var pendingExp = 0;
    final ledger = StorageService.replayLedger;
    final text = await decryptIncoming(payload, fromPeerId: from,
      claimOverride: ledger == null ? null : (rid, exp) async {
        if (_replayInFlight.contains(rid) || await ledger.seen(rid)) return false;
        _replayInFlight.add(rid);
        pendingId = rid;
        pendingExp = exp;
        return true;
      },
    );
    if (text == null) return;

    _chatCtrl.add(IncomingChatEvent(
      fromPeerId: from,
      timestamp:  DateTime.fromMillisecondsSinceEpoch(tsMs),
      text:       text,
      commit: () async {
        final rid = pendingId;
        if (rid == null || ledger == null) return true;
        try {
          await ledger.claim(rid, pendingExp);
          return true;
        } catch (_) {
          return false;
        } finally {
          _replayInFlight.remove(rid);
        }
      },
    ));
  }

  /// Предел DateTime в Dart (±100 000 000 суток от 1970 г.).
  static const _maxDateMs = 8640000000000000;

  /// [1B 0x13][32B от кого][8B время][4B длина][зашифрованный сигнал]. Открытый текст не принимаем: сигналы звонка только E2E.
  Future<void> _handleLiveMsg(Uint8List data) async {
    if (data.length < 45) return;
    final from = _bytesToHex(data.sublist(1, 33));
    final tsMs = ByteData.sublistView(data, 33, 41).getInt64(0, Endian.little);
    final pLen = ByteData.sublistView(data, 41, 45).getUint32(0, Endian.little);
    if (data.length < 45 + pLen) return;
    final payload = data.sublist(45, 45 + pLen);
    if (tsMs < 0 || tsMs > _maxDateMs) return;   // кривое время в заголовке: отбрасываем до расшифровки (журнал повторов не тратим)
    final text = await decryptIncoming(payload, fromPeerId: from, kind: 2);
    if (text == null) return;
    _liveCtrl.add(LiveSignalEvent(fromPeerId: from, timestamp: DateTime.fromMillisecondsSinceEpoch(tsMs), text: text));
  }

  void _handlePeerStatus(Uint8List data) {
    if (data.length < 34) return;
    final peerId = _bytesToHex(data.sublist(1, 33));
    final online = data[33] != 0;
    _statusCtrl.add(PeerStatusEvent(peerId, online));
  }

  // ── Отправка ──────────────────────────────────────────────────────────────

  /// Sends only an encrypted payload. Missing recipient keys are a failure;
  /// there is intentionally no plaintext fallback.
  Future<bool> sendMessage(String toPeerIdHex, String text) async {
    if (_channel == null) return false;
    // Получаем публичный X25519 ключ получателя (из кэша или с ноды)
    final recipientPub = await _getRecipientPub(toPeerIdHex);
    if (recipientPub == null) return false;

    final payload = await _encrypt(text,recipientPub,toPeerIdHex,1);
    if (payload == null) return false;

    final peerBytes = _hexToBytes(toPeerIdHex);
    final buf = BytesBuilder();
    buf.addByte(ftSendMsg);
    buf.add(peerBytes);
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    return _sendRaw(buf.toBytes());
  }

  /// Отправить только зашифрованно. Если ключа получателя нет или сокет не подключён, не отправляет ничего и возвращает false
  /// (никогда не откатывается на открытый текст: так передаются ключи от файлов).
  Future<bool> sendEncryptedOnly(String toPeerIdHex, String text) async {
    if (_channel == null) return false;
    final recipientPub = await _getRecipientPub(toPeerIdHex);
    if (recipientPub == null) return false;
    final payload = await _encrypt(text,recipientPub,toPeerIdHex,1);
    if (payload == null) return false;
    final buf = BytesBuilder();
    buf.addByte(ftSendMsg);
    buf.add(_hexToBytes(toPeerIdHex));
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    return _sendRaw(buf.toBytes());
  }

  /// Живой сигнал (звонок): шифруется для получателя, уходит только если он на связи, нигде не хранится.
  /// false — нет сокета или ключа получателя. Если получатель не на связи, придёт [offlineStream].
  Future<bool> sendLive(String toPeerIdHex, String text) async {
    if (_channel == null) return false;
    final recipientPub = await _getRecipientPub(toPeerIdHex);
    if (recipientPub == null) return false;
    final payload = await _encrypt(text,recipientPub,toPeerIdHex,2);
    if (payload == null) return false;
    final buf = BytesBuilder();
    buf.addByte(ftSendLive);
    buf.add(_hexToBytes(toPeerIdHex));
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    return _sendRaw(buf.toBytes());
  }

  /// Есть ли ключ получателя (то есть это своё устройство и E2E возможен).
  Future<bool> canEncryptTo(String peerId) async => (await _getRecipientPub(peerId)) != null;

  /// Единственный вход для входящих блобов (сокет и почтовый ящик): только E2E-конверт текущего формата, с общим окном повторов.
  /// null — не E2E (открытый текст, старый формат 0xE2), не расшифровалось или повтор: такое пользователю не показываем.
  Future<String?> decryptIncoming(List<int> payload, {required String fromPeerId, int kind = 1, void Function()? onDuplicate,
      Future<bool> Function(String id, int expiresMs)? claimOverride}) async {
    try {
    if (!E2ECrypto.isEncrypted(payload)) return null;
    final bundle = await _getPeerBundle?.call(fromPeerId);
    final self = await _ownPeerId?.call();
    final ledger = StorageService.replayLedger;
    if (bundle == null || self == null || ledger == null) return null;
    return E2ECrypto.decrypt(
      Uint8List.fromList(payload),
      (theirPub) => _identity.ecdh(theirPub),
      from: fromPeerId, to: self, kind: kind, senderEd: bundle.ed25519,
      claimReplay: claimOverride ?? ledger.claim,
      onDuplicate: onDuplicate,
    );
    } catch (_) { return null; }
  }

  Future<List<int>?> _getRecipientPub(String peerId) async {
    // Never bypass the pinned signing identity through the old X-only cache.
    return (await _getPeerBundle?.call(peerId))?.x25519;
  }

  Future<List<int>?> _encrypt(String text,List<int> pub,String to,int kind) async {
    try {
      final self = await _ownPeerId?.call();
      if (self == null) return null;
      return await E2ECrypto.encrypt(text,pub,from:self,to:to,kind:kind,sign:_identity.sign);
    } catch (_) { return null; }
  }

  /// false — сокета нет (пропал, пока шифровали) или запись в него не удалась: отправитель не должен считать сообщение ушедшим.
  bool _sendRaw(Uint8List bytes) {
    final ch = _channel;
    if (ch == null) return false;
    try {
      ch.sink.add(bytes);
      return true;
    } catch (_) {
      return false;
    }
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────────

  void disconnect() {
    _pingTimer?.cancel();
    _reconnectTimer?.cancel();
    _channel?.sink.close();
    _channel = null;
  }

  void dispose() {
    disconnect();
    _chatCtrl.close();
    _statusCtrl.close();
    _liveCtrl.close();
    _offlineCtrl.close();
    _groupCtrl.close();
    _profileCtrl.close();
  }

  // ── TLS pinning ────────────────────────────────────────────────────────────

  static HttpClient _buildPinnedClient(String expectedFp) {
    final normalizedFp = expectedFp.trim().toLowerCase();
    if (normalizedFp.isEmpty) {
      throw StateError('TLS certificate fingerprint is required');
    }
    final ctx = SecurityContext(withTrustedRoots: false);
    return HttpClient(context: ctx)
      ..badCertificateCallback = (X509Certificate cert, String host, int port) {
          final fp = crypto.sha256.convert(cert.der).toString();
          return fp.toLowerCase() == normalizedFp;
        };
  }

  static String _bytesToHex(List<int> bytes) =>
      bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();

  static Uint8List _hexToBytes(String hex) {
    final result = Uint8List(hex.length ~/ 2);
    for (int i = 0; i < result.length; i++) {
      result[i] = int.parse(hex.substring(i * 2, i * 2 + 2), radix: 16);
    }
    return result;
  }

  static Uint8List _uint32LE(int v) {
    final b = ByteData(4)..setUint32(0, v, Endian.little);
    return b.buffer.asUint8List();
  }
}
