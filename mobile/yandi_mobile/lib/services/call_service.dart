import 'dart:async';
import 'dart:math';
import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:wakelock_plus/wakelock_plus.dart';
import '../models/call_signal.dart';

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

  CallService({
    required this.sendSignal,
    required this.fetchTurn,
    required this.nodeHost,
    required this.nameOf,
    required this.showScreen,
    required this.onIncomingNotify,
    required this.onIncomingCancel,
    required this.onLog,
  });

  CallPhase phase = CallPhase.idle;
  String? peerId;
  String? cid;
  bool video    = false;
  bool incoming = false;
  String endReason = '';
  bool muted = false, speaker = false, cameraOn = true, turnMissing = false;
  DateTime? startedAt;

  RTCVideoRenderer? localRenderer;
  RTCVideoRenderer? remoteRenderer;

  RTCPeerConnection? _pc;
  MediaStream? _local;
  final List<RTCIceCandidate> _pendingIce = [];
  bool _remoteSet = false;
  Timer? _ringTimer, _lostTimer, _endTimer;
  final AudioPlayer _ring = AudioPlayer();
  bool _ringing = false;

  bool get busy => phase != CallPhase.idle;
  String get peerName => peerId == null ? '' : nameOf(peerId!);
  Duration get elapsed => startedAt == null ? Duration.zero : DateTime.now().difference(startedAt!);

  static String _newCid() {
    final r = Random.secure();
    return List<int>.generate(8, (_) => r.nextInt(256)).map((b) => b.toRadixString(16).padLeft(2, '0')).join();
  }

  // ── Исходящий ──────────────────────────────────────────────────────────────

  Future<String?> start(String toPeerId, {required bool withVideo}) async {
    if (busy) return 'Вы уже разговариваете';
    _reset();
    peerId = toPeerId; cid = _newCid(); video = withVideo; incoming = false;
    phase = CallPhase.calling; endReason = '';
    speaker = withVideo; cameraOn = withVideo;
    notifyListeners();
    showScreen();
    final ok = await sendSignal(toPeerId, CallSignal.now('invite', cid!, video: video).encode());
    if (!ok) {
      _finish('Нет связи с узлом или это не ваше устройство', log: false);
      return null;
    }
    _playRing('sounds/call_out.mp3');
    _ringTimer = Timer(const Duration(seconds: 45), () {
      _signal('hangup');
      _finish('Нет ответа', log: true);
    });
    return null;
  }

  // ── Входящий ───────────────────────────────────────────────────────────────

  /// Сигнал от другого телефона (уже расшифрованный).
  Future<void> onSignal(String from, CallSignal s) async {
    if (!s.fresh) return;
    switch (s.type) {
      case 'invite':
        if (busy) {
          // встречные звонки друг другу: побеждает звонок с меньшим номером, второй уступает и принимается автоматически
          if (phase == CallPhase.calling && from == peerId && s.cid.compareTo(cid!) < 0) {
            _stopRing(); _ringTimer?.cancel();
            cid = s.cid; video = s.video; incoming = true; phase = CallPhase.ringing;
            notifyListeners();
            await accept();
          } else if (!(from == peerId && s.cid == cid)) {
            await sendSignal(from, CallSignal.now('busy', s.cid).encode());
          }
          return;
        }
        _reset();
        peerId = from; cid = s.cid; video = s.video; incoming = true;
        phase = CallPhase.ringing; endReason = '';
        speaker = s.video; cameraOn = s.video;
        notifyListeners();
        onIncomingNotify(from, nameOf(from), video);
        _playRing('sounds/calling.mp3');
        _ringTimer = Timer(const Duration(seconds: 45), () => _finish('Пропущенный звонок', log: true));
        showScreen();
      case 'accept':
        if (_mine(from, s) && !incoming && phase == CallPhase.calling) {
          _stopRing(); _ringTimer?.cancel();
          phase = CallPhase.connecting; notifyListeners();
          await _startMedia(offerer: true);
        }
      case 'offer':
        if (_mine(from, s) && incoming && s.sdp != null) await _onOffer(s.sdp!);
      case 'answer':
        if (_mine(from, s) && !incoming && s.sdp != null) {
          await _pc?.setRemoteDescription(RTCSessionDescription(s.sdp, 'answer'));
          await _remoteReady();
        }
      case 'ice':
        if (_mine(from, s) && s.cand != null) await _onRemoteIce(s.cand!);
      case 'reject':
        if (_mine(from, s)) _finish('Звонок отклонён', log: true);
      case 'busy':
        if (_mine(from, s)) _finish('Абонент занят', log: true);
      case 'hangup':
        if (_mine(from, s)) _finish(phase == CallPhase.ringing ? 'Пропущенный звонок' : 'Собеседник завершил звонок', log: true);
    }
  }

  /// Узел сообщил, что устройство не на связи.
  void onPeerOffline(String id) {
    if (id == peerId && phase == CallPhase.calling) _finish('Абонент не в сети', log: true);
  }

  bool _mine(String from, CallSignal s) => from == peerId && s.cid == cid && phase != CallPhase.idle && phase != CallPhase.ended;

  Future<void> accept() async {
    if (phase != CallPhase.ringing || !incoming) return;
    _stopRing(); _ringTimer?.cancel(); onIncomingCancel();
    phase = CallPhase.connecting; notifyListeners();
    final ok = await sendSignal(peerId!, CallSignal.now('accept', cid!, video: video).encode());
    if (!ok) { _finish('Нет связи с узлом', log: false); return; }
    await _startMedia(offerer: false);
  }

  Future<void> reject() async {
    if (phase != CallPhase.ringing) return;
    await sendSignal(peerId!, CallSignal.now('reject', cid!).encode());
    _finish('Звонок отклонён', log: true);
  }

  Future<void> hangup() async {
    if (!busy || phase == CallPhase.ended) return;
    await _signal('hangup');
    _finish(phase == CallPhase.calling ? 'Звонок отменён' : 'Звонок завершён', log: true);
  }

  Future<bool> _signal(String type, {String? sdp, Map<String, dynamic>? cand}) {
    if (peerId == null || cid == null) return Future.value(false);
    return sendSignal(peerId!, CallSignal.now(type, cid!, video: video, sdp: sdp, cand: cand).encode());
  }

  // ── WebRTC ─────────────────────────────────────────────────────────────────

  Future<Map<String, dynamic>> _iceConfig() async {
    final servers = <Map<String, dynamic>>[];
    final t = await fetchTurn();
    if (t != null) {
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
    return {'iceServers': servers, 'sdpSemantics': 'unified-plan', 'bundlePolicy': 'max-bundle'};
  }

  Future<void> _startMedia({required bool offerer}) async {
    try {
      localRenderer = RTCVideoRenderer();  remoteRenderer = RTCVideoRenderer();
      await localRenderer!.initialize();   await remoteRenderer!.initialize();
      _pc = await createPeerConnection(await _iceConfig());
      _pc!.onIceCandidate = (c) {
        if (c.candidate == null) return;
        _signal('ice', cand: {'candidate': c.candidate, 'sdpMid': c.sdpMid, 'sdpMLineIndex': c.sdpMLineIndex});
      };
      _pc!.onTrack = (e) {
        if (e.streams.isNotEmpty) { remoteRenderer?.srcObject = e.streams.first; notifyListeners(); }
      };
      _pc!.onConnectionState = _onConnState;
      _local = await navigator.mediaDevices.getUserMedia({
        'audio': {'echoCancellation': true, 'noiseSuppression': true, 'autoGainControl': true},
        // старт с умеренного 360p: на слабом канале видео деградирует плавно, а не замирает
        'video': video ? {'facingMode': 'user', 'width': {'ideal': 480}, 'height': {'ideal': 360}, 'frameRate': {'ideal': 20}} : false,
      });
      for (final t in _local!.getTracks()) { await _pc!.addTrack(t, _local!); }
      localRenderer!.srcObject = _local;
      if (video) await _tuneVideoSender();   // лимит битрейта и поведение при нехватке канала
      await Helper.setSpeakerphoneOn(speaker);
      unawaited(WakelockPlus.enable());
      notifyListeners();
      if (offerer) {
        final o = await _pc!.createOffer({'offerToReceiveAudio': 1, 'offerToReceiveVideo': video ? 1 : 0});
        await _pc!.setLocalDescription(o);
        if (!await _signal('offer', sdp: o.sdp)) _finish('Нет связи с узлом', log: false);
      }
    } catch (e) {
      _finish('Не удалось начать звонок: ${_short(e)}', log: false);
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
    // предложение может прийти раньше, чем готов микрофон: ждём соединение
    for (var i = 0; i < 100 && _pc == null && busy; i++) { await Future<void>.delayed(const Duration(milliseconds: 100)); }
    final pc = _pc;
    if (pc == null) return;
    try {
      await pc.setRemoteDescription(RTCSessionDescription(sdp, 'offer'));
      await _remoteReady();
      final a = await pc.createAnswer({'offerToReceiveAudio': 1, 'offerToReceiveVideo': video ? 1 : 0});
      await pc.setLocalDescription(a);
      if (!await _signal('answer', sdp: a.sdp)) _finish('Нет связи с узлом', log: false);
    } catch (e) {
      _finish('Не удалось ответить: ${_short(e)}', log: false);
    }
  }

  Future<void> _remoteReady() async {
    _remoteSet = true;
    for (final c in _pendingIce) { await _pc?.addCandidate(c); }
    _pendingIce.clear();
  }

  Future<void> _onRemoteIce(Map<String, dynamic> m) async {
    final c = RTCIceCandidate(m['candidate'] as String?, m['sdpMid'] as String?, (m['sdpMLineIndex'] as num?)?.toInt());
    if (_pc != null && _remoteSet) {
      await _pc!.addCandidate(c);
    } else {
      _pendingIce.add(c);
    }
  }

  void _onConnState(RTCPeerConnectionState s) {
    if (s == RTCPeerConnectionState.RTCPeerConnectionStateConnected) {
      _lostTimer?.cancel();
      if (phase != CallPhase.active) {
        phase = CallPhase.active; startedAt = DateTime.now();
        notifyListeners();
      }
    } else if (s == RTCPeerConnectionState.RTCPeerConnectionStateDisconnected) {
      _lostTimer?.cancel();
      _lostTimer = Timer(const Duration(seconds: 10), () => _finish('Связь потеряна', log: true));
    } else if (s == RTCPeerConnectionState.RTCPeerConnectionStateFailed) {
      _finish(turnMissing
          ? 'Не удалось соединиться. Сервер звонков на узле не настроен: без него звонок работает только в одной сети'
          : 'Не удалось соединиться', log: true);
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
    _ringTimer?.cancel(); _lostTimer?.cancel(); _stopRing(); onIncomingCancel();
    final wasActive = phase == CallPhase.active;
    final dur = elapsed;
    final p = peerId, out = !incoming, v = video;
    if (log && p != null) {
      final what = v ? 'Видеозвонок' : 'Звонок';
      final text = wasActive
          ? '📞 $what · ${_fmt(dur)}'
          : (out ? '📞 $what: $reason' : '📞 $reason');
      onLog(p, out, text);
    }
    endReason = reason;
    phase = CallPhase.ended;
    notifyListeners();
    unawaited(_closeMedia());
    _endTimer?.cancel();
    _endTimer = Timer(const Duration(milliseconds: 1800), () { _reset(); notifyListeners(); });
  }

  Future<void> _closeMedia() async {
    try { for (final t in _local?.getTracks() ?? <MediaStreamTrack>[]) { await t.stop(); } } catch (_) {}
    try { await _local?.dispose(); } catch (_) {}
    try { await _pc?.close(); } catch (_) {}
    try { await localRenderer?.dispose(); } catch (_) {}
    try { await remoteRenderer?.dispose(); } catch (_) {}
    try { await Helper.setSpeakerphoneOn(false); } catch (_) {}
    unawaited(WakelockPlus.disable());
    _local = null; _pc = null; localRenderer = null; remoteRenderer = null;
  }

  void _reset() {
    _endTimer?.cancel();
    phase = CallPhase.idle; peerId = null; cid = null; incoming = false; video = false;
    muted = false; startedAt = null; _remoteSet = false; _pendingIce.clear(); turnMissing = false;
  }

  Future<void> _playRing(String asset) async {
    try {
      _ringing = true;
      await _ring.setReleaseMode(ReleaseMode.loop);
      await _ring.play(AssetSource(asset));
    } catch (_) {}
  }

  void _stopRing() {
    if (!_ringing) return;
    _ringing = false;
    unawaited(_ring.stop());
  }

  static String _fmt(Duration d) {
    final m = d.inMinutes.toString().padLeft(2, '0'), s = (d.inSeconds % 60).toString().padLeft(2, '0');
    return d.inHours > 0 ? '${d.inHours}:$m:$s' : '$m:$s';
  }

  static String formatDuration(Duration d) => _fmt(d);
  static String _short(Object e) => e.toString().replaceAll(RegExp(r'\s+'), ' ').substring(0, min(120, e.toString().length));

  @override
  void dispose() {
    _ring.dispose();
    super.dispose();
  }
}
