import 'dart:async';
import 'package:flutter/foundation.dart';
import '../transport/api.dart';
import 'call_signaling.dart';

enum CallPhase {
  idle,
  incomingRinging,
  outgoingRinging,
  connecting,
  active,
  ending,
  ended,
  error,
}

class MediaEvent {
  final String type;
  final Map<String, dynamic>? signal;
  const MediaEvent(this.type, [this.signal]);
}

/// Adapter owns tracks/PC/renderers; close is idempotent, including late open.
abstract class CallMedia {
  Stream<MediaEvent> get events;
  Future<void> open(CallRef call, Map<String, dynamic>? turn);
  Future<void> offer();
  Future<void> receive(Map<String, dynamic> signal);
  Future<void> mute(bool value);
  Future<void> camera(bool value);
  void close();
}

class UnavailableMedia implements CallMedia {
  @override
  Stream<MediaEvent> get events => const Stream.empty();
  @override
  Future<void> open(
    CallRef call,
    Map<String, dynamic>? turn,
  ) async => throw const FormatException(
    'Медиа недоступно: нативная библиотека WebRTC отсутствует в офлайн-кэше.',
  );
  @override
  Future<void> offer() async {}
  @override
  Future<void> receive(Map<String, dynamic> signal) async {}
  @override
  Future<void> mute(bool value) async {}
  @override
  Future<void> camera(bool value) async {}
  @override
  void close() {}
}

/// One owner for node/phone, voice/video. Every async operation belongs to epoch.
class CallCoordinator extends ChangeNotifier {
  final CallSignaling signaling;
  final CallMedia Function() mediaFactory;
  final Future<bool> Function() authorize;
  final Duration ringTimeout, connectTimeout, graceTimeout, endedDelay;
  CallPhase phase = CallPhase.idle;
  CallRef? call;
  String? error, mediaNote;
  bool muted = false, cameraEnabled = false, busy = false;
  int seconds = 0, epoch = 0;
  double? microphoneLevel;
  bool disposed = false,
      polling = false,
      mediaReady = false,
      remoteDescription = false;
  CallMedia? media;
  final ice = <Map<String, dynamic>>[];
  final pending = <Map<String, dynamic>>[];
  Timer? deadline, grace, duration, reset;
  StreamSubscription<CallSignal>? signalSubscription;
  StreamSubscription<MediaEvent>? mediaSubscription;
  Future<void> serial = Future.value();
  CallCoordinator({
    required this.signaling,
    required this.authorize,
    CallMedia Function()? mediaFactory,
    this.ringTimeout = const Duration(seconds: 45),
    this.connectTimeout = const Duration(seconds: 30),
    this.graceTimeout = const Duration(seconds: 10),
    this.endedDelay = const Duration(seconds: 2),
  }) : mediaFactory = mediaFactory ?? UnavailableMedia.new {
    signalSubscription = signaling.signals.listen((s) {
      final own = epoch;
      serial = serial
          .then((_) async {
            if (valid(own)) await handle(s);
          })
          .catchError((Object e) {
            if (valid(own)) finish(e);
          });
    });
  }
  bool get running => const [
    CallPhase.incomingRinging,
    CallPhase.outgoingRinging,
    CallPhase.connecting,
    CallPhase.active,
    CallPhase.ending,
  ].contains(phase);
  bool valid(int own) => !disposed && own == epoch;
  void changed() {
    if (!disposed) notifyListeners();
  }

  Future<void> poll() async {
    if (disposed || polling) return;
    polling = true;
    final own = epoch;
    try {
      final incoming = await signaling.poll();
      if (!valid(own)) return;
      for (final c in incoming) {
        if (disposed) return;
        if (running) {
          if (call?.id != c.id || call?.peer != c.peer)
            await signaling.control(c, 'busy');
        } else if (phase == CallPhase.idle) {
          await receiveCall(c);
        }
      }
    } catch (e) {
      if (valid(own)) {
        if (running) lost();
        error = safe(e);
        changed();
      }
    } finally {
      polling = false;
    }
  }

  void ringing() {
    final own = epoch;
    deadline?.cancel();
    deadline = Timer(ringTimeout, () {
      if (valid(own)) unawaited(end(timeout: true));
    });
  }

  Future<void> start(
    String peer, {
    bool video = false,
    bool phone = false,
  }) async {
    if (disposed || busy || phase != CallPhase.idle) return;
    final own = ++epoch;
    busy = true;
    phase = CallPhase.outgoingRinging;
    error = null;
    changed();
    try {
      if (!await authorize()) {
        if (valid(own)) finish(const ApiFailure('business'));
        return;
      }
      if (!valid(own)) return;
      final c = await signaling.start(peer, video, phone);
      if (!valid(own)) {
        await signaling.control(c, 'hangup');
        return;
      }
      call = c;
      cameraEnabled = video;
      ringing();
      changed();
      await signaling.watch(c);
    } catch (e) {
      if (valid(own)) finish(e);
    } finally {
      if (valid(own)) {
        busy = false;
        changed();
      }
    }
  }

  Future<void> receiveCall(CallRef c) async {
    if (disposed || phase != CallPhase.idle) return;
    final own = ++epoch;
    call = c;
    phase = CallPhase.incomingRinging;
    cameraEnabled = c.video;
    error = null;
    ringing();
    changed();
    try {
      await signaling.watch(c);
    } catch (e) {
      if (valid(own)) finish(e);
    }
  }

  Future<void> accept() async {
    if (disposed || busy || phase != CallPhase.incomingRinging) return;
    final own = epoch;
    busy = true;
    changed();
    try {
      if (!await authorize()) {
        if (valid(own)) finish(const ApiFailure('business'));
        return;
      }
      if (!valid(own)) return;
      await signaling.control(call!, 'accept');
      if (!valid(own)) return;
      unawaited(connect());
    } catch (e) {
      if (valid(own)) finish(e);
    } finally {
      if (valid(own)) {
        busy = false;
        changed();
      }
    }
  }

  Future<void> connect() async {
    if (disposed ||
        call == null ||
        !const [
          CallPhase.incomingRinging,
          CallPhase.outgoingRinging,
        ].contains(phase))
      return;
    final own = epoch;
    final c = call!;
    deadline?.cancel();
    phase = CallPhase.connecting;
    deadline = Timer(connectTimeout, () {
      if (valid(own)) unawaited(end(timeout: true));
    });
    final session = mediaFactory();
    media = session;
    mediaSubscription = session.events.listen((event) {
      if (!valid(own)) return;
      if (event.type == 'connected') {
        deadline?.cancel();
        grace?.cancel();
        grace = null;
        if (phase != CallPhase.active) {
          phase = CallPhase.active;
          duration = Timer.periodic(const Duration(seconds: 1), (_) {
            if (valid(own)) {
              seconds++;
              changed();
            }
          });
        }
        changed();
      } else if (event.type == 'disconnected') {
        lost();
      } else if (event.type == 'failed') {
        finish(const ApiFailure('disconnected'));
      } else if (event.type == 'audio-only' || event.type == 'recvonly') {
        cameraEnabled = false;
        mediaNote = event.type == 'recvonly'
            ? 'Микрофон недоступен: только приём звука и видео.'
            : 'Камера недоступна: передаётся только аудио.';
        changed();
      } else if (event.type == 'level') {
        microphoneLevel = (event.signal?['value'] as num?)?.toDouble();
        changed();
      } else if (event.type == 'preview') {
        changed();
      } else if (event.type == 'signal' && event.signal != null) {
        unawaited(
          signaling.send(c, event.signal!).catchError((Object e) {
            if (valid(own)) finish(e);
          }),
        );
      }
    });
    changed();
    try {
      final turn = c.phone ? await signaling.turn() : null;
      if (!valid(own)) {
        session.close();
        return;
      }
      await session.open(c, turn);
      if (!valid(own)) {
        session.close();
        return;
      }
      mediaReady = true;
      if (!c.incoming) await session.offer();
      if (!valid(own)) return;
      final buffered = List<Map<String, dynamic>>.of(pending);
      pending.clear();
      for (final signal in buffered) {
        await receiveMedia(signal);
        if (!valid(own)) return;
      }
    } catch (e) {
      session.close();
      if (valid(own)) finish(e);
    }
  }

  Future<void> handle(CallSignal signal) async {
    final c = call;
    if (disposed || !running || c == null || signal.peer != c.peer) return;
    final data = signal.data;
    final id = c.phone ? data['cid'] : data['call_id'];
    // Legacy node SDP/ICE has no call_id. Only allow it on this call's bound WS.
    final type = c.phone ? data['t'] : data['type'];
    final mediaType = c.phone
        ? const ['offer', 'answer', 'ice'].contains(type)
        : (c.video
                  ? const ['video-offer', 'video-answer', 'video-ice']
                  : const ['offer', 'answer', 'ice-candidate'])
              .contains(type);
    if (id != c.id && !(id == null && mediaType && !c.phone)) return;
    if (c.phone && (data['video'] != c.video || data['v'] != 1)) return;
    if (type == 'call-accept' || type == 'accept') {
      if (phase == CallPhase.outgoingRinging) {
        final own = epoch;
        if (!await authorize()) {
          if (valid(own)) finish(const ApiFailure('business'));
          return;
        }
        if (valid(own)) unawaited(connect());
      }
    } else if (const [
      'call-reject',
      'reject',
      'busy',
      'hangup',
    ].contains(type)) {
      finish(null);
    } else if (type == 'transport-lost') {
      lost();
    } else if (mediaType &&
        const [CallPhase.connecting, CallPhase.active].contains(phase)) {
      if (!mediaReady) {
        if (pending.length >= 128) {
          finish(const ApiFailure('large'));
          return;
        }
        pending.add(data);
      } else
        await receiveMedia(data);
    }
  }

  Future<void> receiveMedia(Map<String, dynamic> data) async {
    final own = epoch;
    final type = call!.phone ? data['t'] : data['type'];
    if (const ['ice', 'ice-candidate', 'video-ice'].contains(type) &&
        !remoteDescription) {
      if (ice.length >= 128) {
        finish(const ApiFailure('large'));
        return;
      }
      ice.add(data);
      return;
    }
    await media!.receive(data);
    if (!valid(own)) return;
    if (const [
      'offer',
      'answer',
      'video-offer',
      'video-answer',
    ].contains(type)) {
      remoteDescription = true;
      final buffered = List<Map<String, dynamic>>.of(ice);
      ice.clear();
      for (final candidate in buffered) {
        await media!.receive(candidate);
        if (!valid(own)) return;
      }
    }
  }

  void lost() {
    if (disposed || !running || grace != null) return;
    final own = epoch;
    grace = Timer(graceTimeout, () {
      if (valid(own)) unawaited(end(timeout: true));
    });
  }

  Future<void> mute() async {
    if (disposed || busy || phase != CallPhase.active) return;
    final own = epoch;
    busy = true;
    changed();
    try {
      await media!.mute(!muted);
      if (valid(own)) muted = !muted;
    } catch (e) {
      if (valid(own)) finish(e);
    } finally {
      if (valid(own)) {
        busy = false;
        changed();
      }
    }
  }

  Future<void> camera() async {
    if (disposed || busy || phase != CallPhase.active || call?.video != true)
      return;
    final own = epoch;
    busy = true;
    changed();
    try {
      await media!.camera(!cameraEnabled);
      if (valid(own)) cameraEnabled = !cameraEnabled;
    } catch (e) {
      if (valid(own)) finish(e);
    } finally {
      if (valid(own)) {
        busy = false;
        changed();
      }
    }
  }

  Future<void> end({bool reject = false, bool timeout = false}) async {
    if (disposed || !running || phase == CallPhase.ending) return;
    final c = call;
    final own = ++epoch;
    phase = CallPhase.ending;
    cleanup();
    changed();
    try {
      if (c != null)
        await signaling.control(
          c,
          reject || c.incoming && !mediaReady ? 'reject' : 'hangup',
        );
      if (valid(own))
        finish(
          timeout
              ? const FormatException('Время ожидания звонка истекло.')
              : null,
          notifyPeer: false,
        );
    } catch (e) {
      if (valid(own)) finish(e, notifyPeer: false);
    }
  }

  String safe(Object e) => e is FormatException
      ? e.message
      : e is ApiFailure
      ? e.message
      : 'Ошибка сигнального или медиа-слоя.';
  void cleanup() {
    deadline?.cancel();
    grace?.cancel();
    duration?.cancel();
    reset?.cancel();
    deadline = grace = duration = reset = null;
    mediaSubscription?.cancel();
    media?.close();
    media = null;
    signaling.release();
    ice.clear();
    pending.clear();
  }

  void finish(Object? e, {bool notifyPeer = true}) {
    if (disposed) return;
    final c = call;
    if (notifyPeer && running && c != null)
      unawaited(
        signaling
            .control(
              c,
              c.incoming && phase == CallPhase.incomingRinging
                  ? 'reject'
                  : 'hangup',
            )
            .catchError((Object _) {}),
      );
    if (c != null)
      unawaited(
        signaling
            .log(
              c,
              e == null ? '📞 Звонок завершён' : '📞 Звонок завершён с ошибкой',
            )
            .catchError((Object _) {}),
      );
    epoch++;
    cleanup();
    mediaReady = remoteDescription = false;
    busy = false;
    muted = cameraEnabled = false;
    mediaNote = null;
    microphoneLevel = null;
    error = e == null ? null : safe(e);
    phase = e == null ? CallPhase.ended : CallPhase.error;
    changed();
    final own = epoch;
    reset = Timer(endedDelay, () {
      if (valid(own)) {
        call = null;
        seconds = 0;
        phase = CallPhase.idle;
        changed();
      }
    });
  }

  @override
  void dispose() {
    if (disposed) return;
    final c = call;
    if (running && c != null)
      unawaited(
        signaling
            .control(
              c,
              c.incoming && phase == CallPhase.incomingRinging
                  ? 'reject'
                  : 'hangup',
            )
            .catchError((Object _) {}),
      );
    disposed = true;
    epoch++;
    cleanup();
    call = null;
    error = null;
    signalSubscription?.cancel();
    signaling.close();
    super.dispose();
  }
}
