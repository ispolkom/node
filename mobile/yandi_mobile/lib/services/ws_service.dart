import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:crypto/crypto.dart' as crypto;
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:web_socket_channel/io.dart';
import '../models/trusted_node.dart';
import '../crypto/identity.dart';
import '../crypto/e2e_crypto.dart';
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

class IncomingChatEvent {
  final String   fromPeerId;
  final DateTime timestamp;
  final String   text;
  const IncomingChatEvent({required this.fromPeerId, required this.timestamp, required this.text});
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
  final ReplayGuard _replayGuard = ReplayGuard();

  /// Вызывается когда переподключение к текущей ноде не удалось
  /// (задержка достигла максимума). AppState использует это для failover.
  final Future<void> Function()? onPermanentLoss;

  TrustedNode?        _node;
  WebSocketChannel?   _channel;
  Timer?              _pingTimer;
  Timer?              _reconnectTimer;
  int                 _reconnectDelay = 5;
  int                 _reconnectAttempts = 0;
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

  Stream<IncomingChatEvent> get chatStream      => _chatCtrl.stream;
  Stream<PeerStatusEvent>   get statusStream    => _statusCtrl.stream;
  Stream<FileOfferEvent>    get fileOfferStream => _fileOfferCtrl.stream;
  Stream<LiveSignalEvent>   get liveStream      => _liveCtrl.stream;
  /// Номер телефона, которому сигнал не доставлен: его нет на связи.
  Stream<String>            get offlineStream   => _offlineCtrl.stream;
  bool get connected => _channel != null;

  /// Вызывается, когда сокет к узлу реально открыт (в том числе после переподключения): самое время обновить статусы.
  final void Function()? onOpen;

  WsService(this._identity, {this.onPermanentLoss, this.onOpen, Future<List<int>?> Function(String)? getPeerPub})
      : _getPeerPub = getPeerPub;

  final Future<List<int>?> Function(String)? _getPeerPub;

  // ── Управление подключением ────────────────────────────────────────────────

  void connect(TrustedNode node) {
    _node = node;
    _reconnectDelay = 5;
    _tryConnect();
  }

  /// Переключиться на другую ноду (failover).
  void switchNode(TrustedNode node) {
    _pingTimer?.cancel();
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
      ch.ready.then((_) { if (identical(_channel, ch)) onOpen?.call(); }).catchError((_) {});
      ch.stream.listen(
        _onData,
        onError: (_) => _scheduleReconnect(),
        onDone:  ()  => _scheduleReconnect(),
      );
      _startPingTimer();
      _reconnectDelay = 5;
    } catch (_) {
      _scheduleReconnect();
    }
  }

  void _scheduleReconnect() {
    _pingTimer?.cancel();
    _channel = null;
    _reconnectTimer?.cancel();
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
        await _handleLiveMsg(data);

      case ftPeerOffline:
        if (data.length >= 33) _offlineCtrl.add(_bytesToHex(data.sublist(1, 33)));
    }
  }

  Future<void> _handleChatMsg(Uint8List data) async {
    // [1B type][32B from][8B ts_ms_LE][4B payload_len_LE][payload]
    if (data.length < 45) return;
    final from    = _bytesToHex(data.sublist(1, 33));
    final tsMs    = ByteData.sublistView(data, 33, 41).getInt64(0, Endian.little);
    final pLen    = ByteData.sublistView(data, 41, 45).getUint32(0, Endian.little);
    if (data.length < 45 + pLen) return;
    final payload = data.sublist(45, 45 + pLen);

    final text = await decryptIncoming(payload);
    if (text == null) return;

    _chatCtrl.add(IncomingChatEvent(
      fromPeerId: from,
      timestamp:  DateTime.fromMillisecondsSinceEpoch(tsMs),
      text:       text,
    ));
  }

  /// [1B 0x13][32B от кого][8B время][4B длина][зашифрованный сигнал]. Открытый текст не принимаем: сигналы звонка только E2E.
  Future<void> _handleLiveMsg(Uint8List data) async {
    if (data.length < 45) return;
    final from = _bytesToHex(data.sublist(1, 33));
    final tsMs = ByteData.sublistView(data, 33, 41).getInt64(0, Endian.little);
    final pLen = ByteData.sublistView(data, 41, 45).getUint32(0, Endian.little);
    if (data.length < 45 + pLen) return;
    final payload = data.sublist(45, 45 + pLen);
    final text = await decryptIncoming(payload);
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

    final payload = await E2ECrypto.encrypt(text, recipientPub);

    final peerBytes = _hexToBytes(toPeerIdHex);
    final buf = BytesBuilder();
    buf.addByte(ftSendMsg);
    buf.add(peerBytes);
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    _sendRaw(buf.toBytes());
    return true;
  }

  /// Отправить только зашифрованно. Если ключа получателя нет или сокет не подключён, не отправляет ничего и возвращает false
  /// (никогда не откатывается на открытый текст: так передаются ключи от файлов).
  Future<bool> sendEncryptedOnly(String toPeerIdHex, String text) async {
    if (_channel == null) return false;
    final recipientPub = await _getRecipientPub(toPeerIdHex);
    if (recipientPub == null) return false;
    final payload = await E2ECrypto.encrypt(text, recipientPub);
    final buf = BytesBuilder();
    buf.addByte(ftSendMsg);
    buf.add(_hexToBytes(toPeerIdHex));
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    _sendRaw(buf.toBytes());
    return true;
  }

  /// Живой сигнал (звонок): шифруется для получателя, уходит только если он на связи, нигде не хранится.
  /// false — нет сокета или ключа получателя. Если получатель не на связи, придёт [offlineStream].
  Future<bool> sendLive(String toPeerIdHex, String text) async {
    if (_channel == null) return false;
    final recipientPub = await _getRecipientPub(toPeerIdHex);
    if (recipientPub == null) return false;
    final payload = await E2ECrypto.encrypt(text, recipientPub);
    final buf = BytesBuilder();
    buf.addByte(ftSendLive);
    buf.add(_hexToBytes(toPeerIdHex));
    buf.add(_uint32LE(payload.length));
    buf.add(payload);
    _sendRaw(buf.toBytes());
    return true;
  }

  /// Есть ли ключ получателя (то есть это своё устройство и E2E возможен).
  Future<bool> canEncryptTo(String peerId) async => (await _getRecipientPub(peerId)) != null;

  /// Единственный вход для входящих блобов (сокет и почтовый ящик): только E2E-конверт текущего формата, с общим окном повторов.
  /// null — не E2E (открытый текст, старый формат 0xE2), не расшифровалось или повтор: такое пользователю не показываем.
  Future<String?> decryptIncoming(List<int> payload) async {
    if (!E2ECrypto.isEncrypted(payload)) return null;
    return E2ECrypto.decrypt(
      Uint8List.fromList(payload),
      (theirPub) => _identity.ecdh(theirPub),
      replayGuard: _replayGuard,
    );
  }

  Future<List<int>?> _getRecipientPub(String peerId) async {
    // Кэш SQLite (TTL 24ч) содержит только ключи из связок с проверенной подписью: пишет его лишь ApiService.getPeerX25519Pub после
    // проверки, а непроверенные записи старых сборок стёрты при обновлении базы (версия 4).
    final cached = await StorageService.getPeerX25519Pub(peerId);
    if (cached != null) return base64.decode(cached);

    // ключ есть только у других телефонов владельца; у обычных собеседников нода отвечает 404 — тогда текст идёт внутри TLS
    return _getPeerPub == null ? null : await _getPeerPub!(peerId);
  }

  void _sendRaw(Uint8List bytes) => _channel?.sink.add(bytes);

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
