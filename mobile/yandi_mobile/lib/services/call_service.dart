import 'dart:async';
import 'dart:math';
import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:wakelock_plus/wakelock_plus.dart';
import '../models/call_signal.dart';
import '../l10n_core.dart';
import 'app_sounds.dart';

enum CallPhase { idle, calling, ringing, connecting, active, ended }

/// Звонок и видеозвонок между своими устройствами.
///
/// Звук и видео идут по WebRTC и шифруются между самими телефонами (DTLS-SRTP); описания сеанса и сетевые кандидаты едут «живыми»
/// сигналами, зашифрованными телефоном для телефона (см. [CallSignal]). Через сервер звонков (TURN, coturn на компьютере владельца)
/// проходят только уже зашифрованные пакеты. Если телефоны в одной сети, сервер не нужен.
class CallService extends ChangeNotifier {
  /// Отправить сигнал: false — нет связи с узлом или ключа собеседника.
  final Future<bool> Function(String peerId, String text) sendSignal;
  /// Учётные данные сервера звонков (или null, если не настроен).
  final Future<Map<String, dynamic>?> Function() fetchTurn;
  /// Адрес узла (он же адрес сервера звонков, если в настройке узла не задан другой).
  final String? Function() nodeHost;
  final String Function(String peerId) nameOf;
  /// Показать экран звонка / системное уведомление о входящем / записать итог в чат.
  final void Function() showScreen;
  final void Function(String peerId, String name, bool video) onIncomingNotify;
  final void Function() onIncomingCancel;
  final void Function(String peerId, bool outgoing, String text) onLog;
  final bool Function(String kind)? soundEnabled;
  final Future<RTCPeerConnection> Function(Map<String, dynamic>) createPeer;
  final Future<MediaStream> Function(Map<String, dynamic>) acquireMedia;
  final RTCVideoRenderer Function() createRenderer;

  CallService({
    required this.sendSignal,
    required this.fetchTurn,
    required this.nodeHost,
    required this.nameOf,
    required this.showScreen,
    required this.onIncomingNotify,
    required this.onIncomingCancel,
    required this.onLog,
    this.soundEnabled,
    AudioPlayer? ringPlayer,
    Future<RTCPeerConnection> Function(Map<String, dynamic>)? createPeer,
    Future<MediaStream> Function(Map<String, dynamic>)? acquireMedia,
    RTCVideoRenderer Function()? createRenderer,
  }) : _ring = ringPlayer ?? AudioPlayer(),
       createPeer = createPeer ?? ((config) => createPeerConnection(config)),
       acquireMedia = acquireMedia ?? ((constraints) => navigator.mediaDevices.getUserMedia(constraints)),
       createRenderer = createRenderer ?? RTCVideoRenderer.new;

  CallPhase phase = CallPhase.idle;
  String? peerId;
  String? cid;
  bool video    = false;
  bool incoming = false;
  String endReason = '';
  bool muted = false, speaker = false, cameraOn = true, turnMissing = false;
  DateTime? startedAt;
  /// Гость: только аудио (видео закрыто). Выставляет AppState по роли устройства.
  bool audioOnly = false;
  /// Лимит звонка гостя (узел выдаёт доступ к серверу звонков на 15 минут): отсчёт от выдачи доступа.
  Duration? _limit;
  DateTime? _limitFrom;
  Timer? _limitTimer, _warnTimer;
  /// Узел отказал в доступе к серверу звонков (лимит суток / занято): готовая фраза для экрана.
  String? refusal;
  /// Предупреждение «через минуту звонок завершится» (только у гостя).
  String limitWarning = '';

  RTCVideoRenderer? localRenderer;
  RTCVideoRenderer? remoteRenderer;

  RTCPeerConnection? _pc;
  MediaStream? _local;
  final List<RTCIceCandidate> _pendingIce = [];
  bool _remoteSet = false;
  bool _mediaReady = false;
  Future<void> _mediaClosing = Future.value();
  Future<void> _mediaSignals = Future.value();
  Timer? _ringTimer, _lostTimer, _endTimer, _connectTimer, _inviteRetry;
  bool _disposed = false;
  bool _inviteAcknowledged = false;
  final _closedCalls = <String, DateTime>{};
  void _rememberClosed(String peer, String id) {
    _closedCalls.removeWhere((_, until) => until.isBefore(DateTime.now()));
    if (_closedCalls.length >= 256)
      _closedCalls.remove(_closedCalls.keys.first);
    _closedCalls['$peer/$id'] = DateTime.now().add(const Duration(seconds: 90));
  }

  bool _wasClosed(String peer, String id) =>
      _closedCalls['$peer/$id']?.isAfter(DateTime.now()) ?? false;

  void _retryInvite(String peer, String id, bool withVideo) {
    var attempts = 0;
    bool sending = false;
    _inviteRetry = Timer.periodic(const Duration(seconds: 2), (timer) async {
      if (_disposed ||
          cid != id ||
          phase != CallPhase.calling ||
          attempts >= 10) {
        timer.cancel();
        return;
      }
      if (sending) return;
      sending = true;
      attempts++;
      try {
        await _send(
            peer, CallSignal.now('invite', id, video: withVideo).encode());
      } catch (_) {
        // Retry transient transport failures until the existing ring deadline.
      } finally {
        sending = false;
      }
    });
  }

  Future<void> _acknowledge(String peer, CallSignal invite) async {
    try {
      await _send(peer,
          CallSignal.now('ringing', invite.cid, video: invite.video).encode());
    } catch (_) {
      // A repeated invite receives a fresh acknowledgement.
    }
  }
  final AudioPlayer _ring;
  bool _ringing = false;
  int _soundGeneration = 0;
  Future<void> _soundWork = Future<void>.value();

  bool get busy => phase != CallPhase.idle;
  String get peerName => peerId == null ? '' : nameOf(peerId!);
  Duration get elapsed => startedAt == null ? Duration.zero : DateTime.now().difference(startedAt!);

  static String _newCid() {
    final r = Random.secure();
    return List<int>.generate(8, (_) => r.nextInt(256)).map((b) => b.toRadixString(16).padLeft(2, '0')).join();
  }

  // ── Исходящий ──────────────────────────────────────────────────────────────

  Future<String?> start(String toPeerId, {required bool withVideo}) async {
    if (_disposed) return null;
    if (busy) return tr('Вы уже разговариваете');
    _reset();
    final wantVideo = withVideo && !audioOnly;
    peerId = toPeerId; cid = _newCid(); video = wantVideo; incoming = false;
    phase = CallPhase.calling; endReason = '';
    speaker = wantVideo; cameraOn = wantVideo;
    notifyListeners();
    showScreen();
    final ownCid = cid!;
    final ok = await _send(toPeerId, CallSignal.now('invite', ownCid, video: video).encode());
    if (_disposed || cid != ownCid || phase != CallPhase.calling) return null;
    if (!ok) {
      _finish(tr('Нет связи с узлом или это не ваше устройство'), log: false);
      return null;
    }
    if (!_inviteAcknowledged) _retryInvite(toPeerId, ownCid, video);
    _playRing('dialing');
    _ringTimer = Timer(const Duration(seconds: 45), () {
      _signal('hangup');
      _finish(tr('Нет ответа'), log: true);
    });
    return null;
  }

  // ── Входящий ───────────────────────────────────────────────────────────────

  /// Сигнал от другого телефона (уже расшифрованный).
  Future<void> onSignal(String from, CallSignal s) {
    if (!const ['offer', 'answer', 'ice'].contains(s.type)) {
      return _handleSignal(from, s).catchError((Object _) {
        if (!_disposed && _mine(from, s)) {
          unawaited(_signal('hangup'));
          _finish(tr('Не удалось соединиться'), log: true);
        }
      });
    }
    final own = cid;
    return _mediaSignals = _mediaSignals.then((_) async {
      if (!_disposed && cid == own && _mine(from, s)) {
        await _handleSignal(from, s);
      }
    }).catchError((Object _) {
      if (!_disposed && cid == own && _mine(from, s)) {
        unawaited(_signal('hangup'));
        _finish(tr('Не удалось соединиться'), log: true);
      }
    });
  }

  Future<void> _handleSignal(String from, CallSignal s) async {
    if (_disposed || !s.fresh) return;
    if (const ['hangup', 'reject', 'busy'].contains(s.type)) {
      _rememberClosed(from, s.cid);
    }
    switch (s.type) {
      case 'invite':
        if (_wasClosed(from, s.cid)) return;
        if (busy) {
          // встречные звонки друг другу: побеждает звонок с меньшим номером, второй уступает и принимается автоматически
          if (phase == CallPhase.calling && from == peerId && s.cid.compareTo(cid!) < 0) {
            _inviteRetry?.cancel();
            _stopRing(); _ringTimer?.cancel();
            // режим (аудио/видео) остаётся тем, на который согласился пользователь: чужое поле video не включает камеру
            cid = s.cid; incoming = true; phase = CallPhase.ringing;
            notifyListeners();
            await accept();
          } else if (!(from == peerId && s.cid == cid)) {
            await _send(from, CallSignal.now('busy', s.cid, video: s.video).encode());
          } else if (incoming) {
            await _acknowledge(from, s);
          }
          return;
        }
        _reset();
        peerId = from; cid = s.cid; video = s.video && !audioOnly; incoming = true;
        phase = CallPhase.ringing; endReason = '';
        speaker = video; cameraOn = video;
        notifyListeners();
        onIncomingNotify(from, nameOf(from), video);
        _playRing(video ? 'videoCalls' : 'calls');
        _ringTimer = Timer(const Duration(seconds: 45), () => _finish(tr('Пропущенный звонок'), log: true));
        showScreen();
        await _acknowledge(from, s);
      case 'ringing':
        if (_mine(from, s) && !incoming) {
          _inviteAcknowledged = true;
          _inviteRetry?.cancel();
        }
      case 'accept':
        if (_mine(from, s) && !incoming && phase == CallPhase.calling) {
          _inviteRetry?.cancel();
          _stopRing(); _ringTimer?.cancel();
          phase = CallPhase.connecting; _armConnectDeadline(); notifyListeners();
          await _startMedia(offerer: true);
        }
      case 'offer':
        if (_mine(from, s) && incoming && s.sdp != null) await _onOffer(s.sdp!);
      case 'answer':
        if (_mine(from, s) && !incoming && s.sdp != null) {
          final connection = _pc;
          await connection?.setRemoteDescription(RTCSessionDescription(s.sdp, 'answer'));
          if (!_disposed && _mine(from, s) && identical(connection, _pc)) await _remoteReady();
        }
      case 'ice':
        if (_mine(from, s) && s.cand != null) await _onRemoteIce(s.cand!);
      case 'reject':
        if (_mine(from, s)) _finish(tr('Звонок отклонён'), log: true);
      case 'busy':
        if (_mine(from, s)) _finish(tr('Абонент занят'), log: true);
      case 'hangup':
        if (_mine(from, s)) _finish(phase == CallPhase.ringing ? tr('Пропущенный звонок') : tr('Собеседник завершил звонок'), log: true);
    }
  }

  /// Узел сообщил, что устройство не на связи.
  void onPeerOffline(String id) {
    if (id == peerId && phase == CallPhase.calling) _finish(tr('Абонент не в сети'), log: true);
  }

  void onSignalingLost() {
    if (!_disposed && const [CallPhase.calling, CallPhase.ringing,
        CallPhase.connecting].contains(phase)) {
      _finish(tr('Нет связи с узлом'), log: true);
    }
  }

  /// Срок установления соединения: собеседник мог принять звонок и не прислать SDP — без срока звонок вечно «соединяется» и держит
  /// приложение от блокировки.
  void _armConnectDeadline() {
    _connectTimer?.cancel();
    _connectTimer = Timer(const Duration(seconds: 45), () {
      if (phase == CallPhase.connecting) {
        unawaited(_signal('hangup'));
        _finish(tr('Не удалось соединиться'), log: true);
      }
    });
  }

  bool _mine(String from, CallSignal s) => from == peerId && s.cid == cid && phase != CallPhase.idle && phase != CallPhase.ended;

  Future<void> accept() async {
    if (phase != CallPhase.ringing || !incoming) return;
    _stopRing(); _ringTimer?.cancel(); onIncomingCancel();
    phase = CallPhase.connecting; _armConnectDeadline(); notifyListeners();
    final own = cid;
    final ok = await _send(peerId!, CallSignal.now('accept', cid!, video: video).encode());
    if (_disposed || cid != own) return;
    if (phase != CallPhase.connecting || !incoming) return;
    if (!ok) { _finish(tr('Нет связи с узлом'), log: false); return; }
    await _startMedia(offerer: false);
  }

  Future<void> reject() async {
    if (phase != CallPhase.ringing) return;
    final own = cid;
    await _send(peerId!, CallSignal.now('reject', cid!).encode());
    if (_disposed || cid != own) return;
    _finish(tr('Звонок отклонён'), log: true);
  }

  Future<void> hangup() async {
    if (!busy || phase == CallPhase.ended) return;
    final sent = _signal('hangup');
    _finish(phase == CallPhase.calling ? tr('Звонок отменён') : tr('Звонок завершён'), log: true);
    await sent;
  }

  Future<bool> _send(String peer, String text) async {
    try { return await sendSignal(peer, text); } catch (_) { return false; }
  }

  Future<bool> _signal(String type, {String? sdp, Map<String, dynamic>? cand}) {
    if (peerId == null || cid == null) return Future.value(false);
    return _send(peerId!, CallSignal.now(type, cid!, video: video, sdp: sdp, cand: cand).encode());
  }

  // ── WebRTC ─────────────────────────────────────────────────────────────────

  Future<Map<String, dynamic>> _iceConfig() async {
    final own = cid;
    final servers = <Map<String, dynamic>>[];
    final t = await fetchTurn();
    if (_disposed || cid != own || phase == CallPhase.ended || phase == CallPhase.idle) return {};
    refusal = null;
    final refused = t?['_refused'];
    if (refused != null) {
      refusal = refused == 'call_day_limit'
          ? tr('Лимит гостевых звонков на сегодня исчерпан (60 минут в сутки)')
          : tr('На узле сейчас много гостевых звонков. Попробуйте позже.');
    }
    // гость звонит только через сервер звонков узла: без него прямое соединение обошло бы лимиты времени (15 минут, 60 в сутки)
    if (audioOnly && t == null) refusal = tr('Звонки гостю недоступны: на узле не настроен сервер звонков');
    final lim = (t?['max_call_secs'] as num?)?.toInt();
    if (lim != null) { _limit = Duration(seconds: lim); _limitFrom = DateTime.now(); }
    if (t != null && refused == null) {
      final host = (t['host'] as String?) ?? nodeHost() ?? '';
      final port = (t['port'] as num?)?.toInt() ?? 3478;
      if (host.isNotEmpty) {
        servers.add({'urls': ['stun:$host:$port']});
        servers.add({
          'urls': ['turn:$host:$port?transport=udp', 'turn:$host:$port?transport=tcp'],
          'username': t['username'],
          'credential': t['credential'],
        });
      }
    }
    turnMissing = servers.isEmpty;
    return {
      'iceServers': servers, 'sdpSemantics': 'unified-plan', 'bundlePolicy': 'max-bundle',
      // у гостя только relay: весь звук идёт через сервер звонков узла, и лимит действует
      if (audioOnly) 'iceTransportPolicy': 'relay',
    };
  }

  Future<void> _startMedia({required bool offerer}) async {
    final own = cid, peer = peerId;
    bool current() => !_disposed && cid == own &&
        phase != CallPhase.idle && phase != CallPhase.ended;
    RTCVideoRenderer? localView, remoteView;
    RTCPeerConnection? pc;
    MediaStream? stream;
    try {
      await _mediaClosing;
      if (!current()) return;
      localView = createRenderer();
      remoteView = createRenderer();
      await localView.initialize();
      if (!current()) return;
      await remoteView.initialize();
      if (!current()) return;
      localRenderer = localView;
      remoteRenderer = remoteView;
      final cfg = await _iceConfig();
      if (!current()) return;
      if (refusal != null) {
        unawaited(_signal('hangup'));
        _finish(refusal!, log: true);
        return;
      }
      pc = await createPeer(cfg);
      if (!current()) return;
      final connection = pc;
      _pc = connection;
      connection.onIceCandidate = (c) {
        if (!current() || c.candidate?.isNotEmpty != true) return;
        unawaited(_signal('ice', cand: {
          'candidate': c.candidate, 'sdpMid': c.sdpMid,
          'sdpMLineIndex': c.sdpMLineIndex,
        }).catchError((Object _) => false));
      };
      connection.onTrack = (e) {
        if (current() && e.streams.isNotEmpty) {
          remoteView?.srcObject = e.streams.first;
          notifyListeners();
        }
      };
      connection.onConnectionState = (state) {
        if (current()) _onConnState(state);
      };
      stream = await acquireMedia({
        'audio': {'echoCancellation': true, 'noiseSuppression': true, 'autoGainControl': true},
        'video': video ? {'facingMode': 'user', 'width': {'ideal': 480},
          'height': {'ideal': 360}, 'frameRate': {'ideal': 20}} : false,
      });
      if (!current()) return;
      final captured = stream;
      _local = captured;
      for (final track in captured.getTracks()) {
        await connection.addTrack(track, captured);
        if (!current()) return;
      }
      localView.srcObject = captured;
      if (video) await _tuneVideoSender();
      if (!current()) return;
      await Helper.setSpeakerphoneOn(speaker);
      if (!current()) return;
      unawaited(WakelockPlus.enable());
      _mediaReady = true;
      notifyListeners();
      if (offerer) {
        final offer = await connection.createOffer({
          'offerToReceiveAudio': 1, 'offerToReceiveVideo': video ? 1 : 0,
        });
        if (!current()) return;
        await connection.setLocalDescription(offer);
        if (!current()) return;
        final sent = await _send(peer!, CallSignal.now('offer', own!,
            video: video, sdp: offer.sdp).encode());
        if (current() && !sent) _finish(tr('Нет связи с узлом'), log: false);
      }
    } catch (_) {
      if (current()) {
        unawaited(_signal('hangup').catchError((Object _) => false));
        _finish(tr('Не удалось соединиться'), log: true);
      }
    } finally {
      if (!current()) {
        await _releaseMedia(stream, pc, localView, remoteView);
      }
    }
  }

  /// Настройка видеопотока под слабый канал: потолок битрейта и приоритет частоты кадров (разрешение падает раньше, чем картинка замирает).
  Future<void> _tuneVideoSender() async {
    try {
      final senders = await _pc?.getSenders() ?? [];
      for (final sender in senders) {
        if (sender.track?.kind != 'video') continue;
        final params = sender.parameters;
        // на слабом канале лучше ронять разрешение, чем фризить
        params.degradationPreference = RTCDegradationPreference.MAINTAIN_FRAMERATE;
        final encodings = params.encodings;
        if (encodings == null || encodings.isEmpty) {
          params.encodings = [RTCRtpEncoding(maxBitrate: 450000, maxFramerate: 20)];
        } else {
          for (final e in encodings) {
            e.maxBitrate = 450000;   // ~450 кбит/с потолок видео
            e.maxFramerate = 20;
          }
        }
        await sender.setParameters(params);
      }
    } catch (_) {
      // не критично: без тюнинга звонок всё равно работает
    }
  }

  Future<void> _onOffer(String sdp) async {
    // предложение принадлежит звонку, при котором пришло: если за время ожидания звонок сменился, оно ни к чему не применяется
    final myCid = cid, myPeer = peerId;
    bool same() => cid == myCid && peerId == myPeer && phase != CallPhase.idle && phase != CallPhase.ended;
    // Предложение может прийти до разрешений и добавления локальных треков.
    // Ответ раньше этого момента согласовал бы приём без передачи звука и видео.
    for (var i = 0; i < 450 && !_mediaReady && same(); i++) { await Future<void>.delayed(const Duration(milliseconds: 100)); }
    final pc = _pc;
    if (pc == null || !_mediaReady || !same()) return;
    try {
      await pc.setRemoteDescription(RTCSessionDescription(sdp, 'offer'));
      if (!same()) return;
      await _remoteReady();
      if (!same()) return;
      final a = await pc.createAnswer({'offerToReceiveAudio': 1, 'offerToReceiveVideo': video ? 1 : 0});
      if (!same()) return;
      await pc.setLocalDescription(a);
      if (!same()) return;
      if (!await _signal('answer', sdp: a.sdp)) { if (same()) _finish(tr('Нет связи с узлом'), log: false); }
    } catch (e) {
      if (same()) _finish(trf('Не удалось ответить: {e}', {'e': _short(e)}), log: false);
    }
  }

  Future<void> _remoteReady() async {
    final connection = _pc, own = cid;
    if (connection == null) return;
    _remoteSet = true;
    final candidates = List<RTCIceCandidate>.of(_pendingIce);
    _pendingIce.clear();
    for (final c in candidates) {
      if (_disposed || cid != own || !identical(_pc, connection)) return;
      await connection.addCandidate(c);
    }
  }

  Future<void> _onRemoteIce(Map<String, dynamic> m) async {
    final c = RTCIceCandidate(m['candidate'] as String?, m['sdpMid'] as String?, (m['sdpMLineIndex'] as num?)?.toInt());
    if (_pc != null && _remoteSet) {
      await _pc!.addCandidate(c);
    } else {
      _pendingIce.add(c);
    }
  }

  /// Гость: звонок обрывается, когда истёк срок доступа к серверу звонков (15 минут от выдачи); за минуту — предупреждение.
  void _armLimit() {
    final lim = _limit, from = _limitFrom;
    if (lim == null || from == null) return;
    final left = lim - DateTime.now().difference(from);
    _limitTimer?.cancel(); _warnTimer?.cancel();
    if (left <= Duration.zero) {
      unawaited(_signal('hangup'));
      _finish(tr('Лимит гостевого звонка: 15 минут'), log: true);
      return;
    }
    _limitTimer = Timer(left, () {
      unawaited(_signal('hangup'));
      _finish(tr('Лимит гостевого звонка: 15 минут'), log: true);
    });
    if (left > const Duration(seconds: 60)) {
      _warnTimer = Timer(left - const Duration(seconds: 60), () {
        limitWarning = tr('Через минуту звонок завершится: лимит гостевого звонка 15 минут');
        notifyListeners();
      });
    } else {
      limitWarning = tr('Через минуту звонок завершится: лимит гостевого звонка 15 минут');
    }
  }

  void _onConnState(RTCPeerConnectionState s) {
    if (s == RTCPeerConnectionState.RTCPeerConnectionStateConnected) {
      _lostTimer?.cancel();
      if (phase != CallPhase.active) {
        _connectTimer?.cancel();
        phase = CallPhase.active; startedAt = DateTime.now();
        _armLimit();
        notifyListeners();
      }
    } else if (s == RTCPeerConnectionState.RTCPeerConnectionStateDisconnected) {
      _lostTimer?.cancel();
      _lostTimer = Timer(const Duration(seconds: 10), () => _finish(tr('Связь потеряна'), log: true));
    } else if (s == RTCPeerConnectionState.RTCPeerConnectionStateFailed) {
      _finish(turnMissing
          ? tr('Не удалось соединиться. Сервер звонков на узле не настроен: без него звонок работает только в одной сети')
          : tr('Не удалось соединиться'), log: true);
    }
  }

  // ── Управление во время звонка ─────────────────────────────────────────────

  void toggleMute() {
    muted = !muted;
    for (final t in _local?.getAudioTracks() ?? <MediaStreamTrack>[]) { t.enabled = !muted; }
    notifyListeners();
  }

  Future<void> toggleSpeaker() async {
    speaker = !speaker;
    await Helper.setSpeakerphoneOn(speaker);
    notifyListeners();
  }

  void toggleCamera() {
    cameraOn = !cameraOn;
    for (final t in _local?.getVideoTracks() ?? <MediaStreamTrack>[]) { t.enabled = cameraOn; }
    notifyListeners();
  }

  Future<void> switchCamera() async {
    final tracks = _local?.getVideoTracks() ?? <MediaStreamTrack>[];
    if (tracks.isNotEmpty) await Helper.switchCamera(tracks.first);
  }

  // ── Завершение ─────────────────────────────────────────────────────────────

  void _finish(String reason, {required bool log}) {
    if (phase == CallPhase.idle || phase == CallPhase.ended) return;
    _inviteRetry?.cancel();
    if (peerId != null && cid != null) _rememberClosed(peerId!, cid!);
    _ringTimer?.cancel(); _lostTimer?.cancel(); _connectTimer?.cancel(); _limitTimer?.cancel(); _warnTimer?.cancel(); _stopRing(); onIncomingCancel();
    final wasActive = phase == CallPhase.active;
    final dur = elapsed;
    final p = peerId, out = !incoming, v = video;
    if (log && p != null) {
      final what = v ? tr('Видеозвонок') : tr('Звонок');
      final text = wasActive
          ? '📞 $what · ${_fmt(dur)}'
          : (out ? '📞 $what: $reason' : '📞 $reason');
      onLog(p, out, text);
    }
    endReason = reason;
    phase = CallPhase.ended;
    notifyListeners();
    _closeMedia();
    _endTimer?.cancel();
    _endTimer = Timer(const Duration(milliseconds: 1800), () { _reset(); notifyListeners(); });
  }

  void _closeMedia() {
    // Detach before awaiting native cleanup: it must never read the next call.
    final stream = _local, connection = _pc;
    final localView = localRenderer, remoteView = remoteRenderer;
    _local = null;
    _pc = null;
    localRenderer = remoteRenderer = null;
    _mediaReady = false;
    _mediaClosing = _mediaClosing.then((_) async {
      await _releaseMedia(stream, connection, localView, remoteView);
      try { await Helper.setSpeakerphoneOn(false); } catch (_) {}
      try { await WakelockPlus.disable(); } catch (_) {}
    });
  }

  static Future<void> _releaseMedia(MediaStream? stream,
      RTCPeerConnection? connection, RTCVideoRenderer? localView,
      RTCVideoRenderer? remoteView) async {
    if (connection != null) {
      connection.onIceCandidate = null;
      connection.onTrack = null;
      connection.onConnectionState = null;
    }
    for (final track in stream?.getTracks() ?? <MediaStreamTrack>[]) {
      try { await track.stop(); } catch (_) {}
    }
    try { await stream?.dispose(); } catch (_) {}
    try { await connection?.close(); } catch (_) {}
    try { await connection?.dispose(); } catch (_) {}
    try { await localView?.dispose(); } catch (_) {}
    try { await remoteView?.dispose(); } catch (_) {}
  }

  void _reset() {
    _inviteAcknowledged = false;
    _mediaSignals = Future.value();
    _inviteRetry?.cancel();
    _endTimer?.cancel();
    _limitTimer?.cancel(); _warnTimer?.cancel(); _limit = null; _limitFrom = null; limitWarning = ''; refusal = null;
    phase = CallPhase.idle; peerId = null; cid = null; incoming = false; video = false;
    muted = false; startedAt = null; _remoteSet = false; _mediaReady = false; _pendingIce.clear(); turnMissing = false;
  }

  Future<void> _playRing(String kind) {
    if (soundEnabled?.call(kind) == false) return Future<void>.value();
    final generation = ++_soundGeneration;
    _ringing = true;
    return _soundWork = _soundWork.catchError((Object _) {}).then((_) async {
      if (generation != _soundGeneration) return;
      await _ring.setReleaseMode(ReleaseMode.loop);
      if (generation != _soundGeneration) return;
      await _ring.setAudioContext(AppSounds.context(kind));
      if (generation != _soundGeneration) return;
      await _ring.play(AssetSource('sounds/${AppSounds.assets[kind]}'));
    }).catchError((Object _) {
      if (generation == _soundGeneration) _ringing = false;
      debugPrint('YANDI: call sound unavailable');
    });
  }

  void refreshRingtone() {
    final kind = phase == CallPhase.calling ? 'dialing'
        : phase == CallPhase.ringing ? (video ? 'videoCalls' : 'calls') : null;
    if (kind == null || soundEnabled?.call(kind) == false) {
      _stopRing();
    } else if (!_ringing) {
      unawaited(_playRing(kind));
    }
  }

  void _stopRing() {
    ++_soundGeneration;
    _ringing = false;
    _soundWork = _soundWork.catchError((Object _) {}).then((_) => _ring.stop());
  }

  static String _fmt(Duration d) {
    final m = d.inMinutes.toString().padLeft(2, '0'), s = (d.inSeconds % 60).toString().padLeft(2, '0');
    return d.inHours > 0 ? '${d.inHours}:$m:$s' : '$m:$s';
  }

  static String formatDuration(Duration d) => _fmt(d);
  static String _short(Object e) {
    final text = e.toString().replaceAll(RegExp(r'\s+'), ' ');
    return text.substring(0, min(120, text.length));
  }

  @override
  void dispose() {
    _disposed = true;
    _closeMedia();
    _inviteRetry?.cancel();
    _ringTimer?.cancel();
    _lostTimer?.cancel();
    _endTimer?.cancel();
    _connectTimer?.cancel();
    _limitTimer?.cancel();
    _warnTimer?.cancel();
    _stopRing();
    unawaited(_soundWork.then((_) => _ring.dispose()));
    super.dispose();
  }
}
