import 'dart:async';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'call_coordinator.dart';
import 'call_signaling.dart';

/// One-shot native session. Late resources are disposed before open completes.
class WebRtcCallMedia implements CallMedia {
  final Future<RTCPeerConnection> Function(Map<String, dynamic>) createPeer;
  final Future<MediaStream> Function(Map<String, dynamic>) acquire;
  final RTCVideoRenderer localRenderer, remoteRenderer;
  final _events = StreamController<MediaEvent>.broadcast();
  RTCPeerConnection? _pc;
  MediaStream? _local;
  final _remote = <MediaStream>{};
  CallRef? _call;
  bool _closed = false;
  bool _localInitialized = false, _remoteInitialized = false;
  Future<void>? _opening;
  Timer? _levelTimer;
  bool _sampling = false;
  Future<void> released = Future.value();

  WebRtcCallMedia({
    Future<RTCPeerConnection> Function(Map<String, dynamic>)? createPeer,
    Future<MediaStream> Function(Map<String, dynamic>)? acquire,
    RTCVideoRenderer? localRenderer,
    RTCVideoRenderer? remoteRenderer,
  }) : createPeer = createPeer ?? ((config) => createPeerConnection(config)),
       acquire =
           acquire ??
           ((constraints) => navigator.mediaDevices.getUserMedia(constraints)),
       localRenderer = localRenderer ?? RTCVideoRenderer(),
       remoteRenderer = remoteRenderer ?? RTCVideoRenderer();

  @override
  Stream<MediaEvent> get events => _events.stream;
  void emit(String type, [Map<String, dynamic>? signal]) {
    if (!_closed) _events.add(MediaEvent(type, signal));
  }

  Map<String, dynamic> configuration(Map<String, dynamic>? turn) => {
    'sdpSemantics': 'unified-plan',
    'bundlePolicy': 'max-bundle',
    'iceServers': [
      {'urls': 'stun:stun.l.google.com:19302'},
      {'urls': 'stun:stun1.l.google.com:19302'},
      if (turn != null)
        {
          'urls': [
            'turn:${turn['host']}:${turn['port']}?transport=udp',
            'turn:${turn['host']}:${turn['port']}?transport=tcp',
          ],
          'username': turn['username'],
          'credential': turn['credential'],
        },
    ],
  };

  @override
  Future<void> open(CallRef call, Map<String, dynamic>? turn) {
    if (_closed || _opening != null) return Future.value();
    _call = call;
    return _opening = _open(call, turn);
  }

  Future<void> _open(CallRef call, Map<String, dynamic>? turn) async {
    try {
      if (call.video) {
        await localRenderer.initialize();
        _localInitialized = true;
        if (_closed) return;
        await remoteRenderer.initialize();
        _remoteInitialized = true;
        if (_closed) return;
      }
      _pc = await createPeer(configuration(turn));
      if (_closed) return;
      _pc!.onIceCandidate = (candidate) {
        if (candidate.candidate?.isNotEmpty != true) return;
        emit('signal', {
          call.phone ? 't' : 'type': call.phone
              ? 'ice'
              : call.video
              ? 'video-ice'
              : 'ice-candidate',
          call.phone ? 'cand' : 'candidate': candidate.toMap(),
        });
      };
      _pc!.onConnectionState = (state) {
        if (_closed) return;
        switch (state) {
          case RTCPeerConnectionState.RTCPeerConnectionStateConnected:
            emit('connected');
            _levelTimer ??= Timer.periodic(
              const Duration(milliseconds: 500),
              (_) => unawaited(_sampleLevel()),
            );
          case RTCPeerConnectionState.RTCPeerConnectionStateDisconnected:
            emit('disconnected');
          case RTCPeerConnectionState.RTCPeerConnectionStateFailed:
            emit('failed');
          default:
            break;
        }
      };
      _pc!.onTrack = (event) {
        if (_closed) return;
        _remote.addAll(event.streams);
        if (_remoteInitialized && event.streams.isNotEmpty) {
          remoteRenderer.srcObject = event.streams.first;
          emit('preview');
        }
      };
      try {
        _local = await acquire({
          'audio': {'echoCancellation': true, 'noiseSuppression': true},
          'video': call.video
              ? {
                  'width': {'ideal': 640},
                  'height': {'ideal': 480},
                }
              : false,
        });
      } catch (_) {
        if (_closed) return;
        if (call.video) {
          try {
            _local = await acquire({'audio': true, 'video': false});
          } catch (_) {
            if (!_closed) emit('recvonly');
          }
        } else {
          emit('recvonly');
        }
      }
      if (_closed) return;
      final tracks = _local?.getTracks() ?? <MediaStreamTrack>[];
      for (final track in tracks) {
        await _pc!.addTrack(track, _local!);
        if (_closed) return;
      }
      if (!tracks.any((t) => t.kind == 'audio')) {
        await _pc!.addTransceiver(
          kind: RTCRtpMediaType.RTCRtpMediaTypeAudio,
          init: RTCRtpTransceiverInit(direction: TransceiverDirection.RecvOnly),
        );
        if (_closed) return;
      }
      if (call.video && !tracks.any((t) => t.kind == 'video')) {
        if (tracks.any((t) => t.kind == 'audio')) emit('audio-only');
        await _pc!.addTransceiver(
          kind: RTCRtpMediaType.RTCRtpMediaTypeVideo,
          init: RTCRtpTransceiverInit(direction: TransceiverDirection.RecvOnly),
        );
        if (_closed) return;
      }
      if (_localInitialized) localRenderer.srcObject = _local;
      if (!tracks.any((t) => t.kind == 'audio')) emit('recvonly');
      emit('preview');
    } finally {
      if (_closed) await _release();
    }
  }

  Future<void> _description(bool offer) async {
    final pc = _pc;
    final call = _call;
    if (_closed || pc == null || call == null) return;
    final sdp = offer ? await pc.createOffer() : await pc.createAnswer();
    if (_closed) return;
    await pc.setLocalDescription(sdp);
    if (_closed) return;
    final type = offer ? 'offer' : 'answer';
    emit('signal', {
      call.phone ? 't' : 'type': call.phone
          ? type
          : call.video
          ? 'video-$type'
          : type,
      'sdp': call.phone ? sdp.sdp : sdp.toMap(),
    });
  }

  @override
  Future<void> offer() => _description(true);
  @override
  Future<void> receive(Map<String, dynamic> signal) async {
    final pc = _pc;
    final call = _call;
    if (_closed || pc == null || call == null) return;
    final type = signal[call.phone ? 't' : 'type'] as String;
    if (const ['ice', 'ice-candidate', 'video-ice'].contains(type)) {
      final candidate = signal[call.phone ? 'cand' : 'candidate'] as Map;
      await pc.addCandidate(
        RTCIceCandidate(
          candidate['candidate'] as String?,
          candidate['sdpMid'] as String?,
          candidate['sdpMLineIndex'] as int?,
        ),
      );
    } else {
      final offer = type == 'offer' || type == 'video-offer';
      final raw = signal['sdp'];
      final sdp = call.phone ? raw as String : (raw as Map)['sdp'] as String;
      await pc.setRemoteDescription(
        RTCSessionDescription(sdp, offer ? 'offer' : 'answer'),
      );
      if (!_closed && offer) await _description(false);
    }
  }

  @override
  Future<void> mute(bool value) async {
    if (_closed) return;
    for (final track in _local?.getAudioTracks() ?? <MediaStreamTrack>[]) {
      track.enabled = !value;
    }
  }

  @override
  Future<void> camera(bool value) async {
    if (_closed) return;
    for (final track in _local?.getVideoTracks() ?? <MediaStreamTrack>[]) {
      track.enabled = value;
    }
  }

  Future<void> _sampleLevel() async {
    final pc = _pc;
    final tracks = _local?.getAudioTracks() ?? <MediaStreamTrack>[];
    if (_closed || _sampling || pc == null || tracks.isEmpty) return;
    _sampling = true;
    try {
      final stats = await pc.getStats(tracks.first);
      if (_closed) return;
      for (final stat in stats) {
        final level = stat.values['audioLevel'];
        if ((stat.type == 'media-source' || stat.type == 'outbound-rtp') &&
            level is num) {
          emit('level', {
            'value': tracks.first.enabled
                ? level.toDouble().clamp(0.0, 1.0)
                : 0.0,
          });
          break;
        }
      }
    } catch (_) {
      /* Some backends do not expose audioLevel. */
    } finally {
      _sampling = false;
    }
  }

  // Each disposal is attempted even if another native resource fails.
  Future<void> _attempt(Future<void> Function() action) async {
    try {
      await action();
    } catch (_) {
      /* No SDP/credentials in logs. */
    }
  }

  Future<void> _release() async {
    final pc = _pc;
    _pc = null;
    if (pc != null) {
      pc.onIceCandidate = null;
      pc.onTrack = null;
      pc.onConnectionState = null;
    }
    if (_localInitialized) {
      _localInitialized = false;
      await _attempt(() => localRenderer.setSrcObject(stream: null));
      await _attempt(localRenderer.dispose);
    }
    if (_remoteInitialized) {
      _remoteInitialized = false;
      await _attempt(() => remoteRenderer.setSrcObject(stream: null));
      await _attempt(remoteRenderer.dispose);
    }
    final streams = {..._remote, if (_local != null) _local!};
    _local = null;
    _remote.clear();
    final tracks = streams.expand((s) => s.getTracks()).toSet();
    for (final track in tracks) {
      await _attempt(track.stop);
    }
    for (final stream in streams) {
      await _attempt(stream.dispose);
    }
    if (pc != null) {
      await _attempt(pc.close);
      await _attempt(pc.dispose);
    }
    _call = null;
  }

  @override
  void close() {
    if (_closed) return;
    _closed = true;
    _levelTimer?.cancel();
    _levelTimer = null;
    // Stop owned resources now, then sweep anything returned by late open.
    released = () async {
      await _release();
      try {
        await _opening;
      } catch (_) {
        /* Already reported by coordinator. */
      }
      await _release();
      await _events.close();
    }();
  }
}
