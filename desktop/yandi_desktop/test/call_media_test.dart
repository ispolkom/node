import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:yandi_desktop/domain/call_media.dart';
import 'package:yandi_desktop/domain/call_signaling.dart';

class Track implements MediaStreamTrack {
  @override
  final String kind;
  @override
  bool enabled = true;
  int stops = 0;
  bool failStop = false;
  Track(this.kind);
  @override
  Future<void> stop() async {
    stops++;
    if (failStop) throw StateError('stop');
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class StreamFake implements MediaStream {
  final List<Track> tracks;
  int disposals = 0;
  StreamFake(this.tracks);
  @override
  List<MediaStreamTrack> getTracks() => tracks;
  @override
  List<MediaStreamTrack> getAudioTracks() =>
      tracks.where((t) => t.kind == 'audio').toList();
  @override
  List<MediaStreamTrack> getVideoTracks() =>
      tracks.where((t) => t.kind == 'video').toList();
  @override
  Future<void> dispose() async {
    disposals++;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class PeerFake extends RTCPeerConnection {
  int closes = 0, disposals = 0, adds = 0;
  final received = <String>[];
  final directions = <TransceiverDirection>[];
  @override
  Future<List<StatsReport>> getStats([MediaStreamTrack? track]) async => [
    StatsReport('microphone', 'media-source', 0, {'audioLevel': 0.4}),
  ];
  @override
  Future<RTCSessionDescription> createOffer([
    Map<String, dynamic>? constraints,
  ]) async => RTCSessionDescription('local offer', 'offer');
  @override
  Future<RTCSessionDescription> createAnswer([
    Map<String, dynamic>? constraints,
  ]) async => RTCSessionDescription('local answer', 'answer');
  @override
  Future<void> setLocalDescription(RTCSessionDescription description) async {}
  @override
  Future<void> setRemoteDescription(RTCSessionDescription description) async {
    received.add('${description.type}:${description.sdp}');
  }

  @override
  Future<void> addCandidate(RTCIceCandidate candidate) async {
    received.add('ice:${candidate.candidate}');
  }

  @override
  Future<RTCRtpSender> addTrack(
    MediaStreamTrack track, [
    MediaStream? stream,
  ]) async {
    adds++;
    return SenderFake();
  }

  @override
  Future<RTCRtpTransceiver> addTransceiver({
    MediaStreamTrack? track,
    RTCRtpMediaType? kind,
    RTCRtpTransceiverInit? init,
  }) async {
    directions.add(init!.direction!);
    return TransceiverFake();
  }

  @override
  Future<void> close() async {
    closes++;
  }

  @override
  Future<void> dispose() async {
    disposals++;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class SenderFake implements RTCRtpSender {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class TransceiverFake implements RTCRtpTransceiver {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class RendererFake extends RTCVideoRenderer {
  int disposals = 0;
  MediaStream? stream;
  Completer<void>? initialization;
  @override
  Future<void> initialize() async {
    await initialization?.future;
  }

  @override
  set srcObject(MediaStream? value) {
    stream = value;
  }

  @override
  MediaStream? get srcObject => stream;
  @override
  Future<void> setSrcObject({MediaStream? stream, String? trackId}) async {
    this.stream = stream;
  }

  @override
  Future<void> dispose() async {
    disposals++;
    await super.dispose();
  }
}

void main() {
  const peer = '0123456789abcdef';
  for (final phone in [false, true]) {
    test(
      'native adapter SDP/ICE envelope phone=$phone and full release',
      () async {
        final pc = PeerFake(), local = RendererFake(), remote = RendererFake();
        final audio = Track('audio'), video = Track('video');
        final stream = StreamFake([audio, video]);
        final incoming = StreamFake([Track('video')]);
        Map<String, dynamic>? config, constraints;
        final media = WebRtcCallMedia(
          createPeer: (c) async {
            config = c;
            return pc;
          },
          acquire: (c) async {
            constraints = c;
            return stream;
          },
          localRenderer: local,
          remoteRenderer: remote,
        );
        final events = <dynamic>[];
        media.events.listen(events.add);
        await media.open(CallRef(peer, 'call0001', phone: phone, video: true), {
          'host': '127.0.0.1',
          'port': 3478,
          'username': 'temporary',
          'credential': 'temporary',
        });
        expect((config!['iceServers'] as List).length, 3);
        expect(constraints!['video']['width']['ideal'], 640);
        expect(constraints!['video']['height']['ideal'], 480);
        expect(pc.adds, 2);
        expect(local.stream, stream);
        pc.onTrack!(
          RTCTrackEvent(streams: [incoming], track: incoming.tracks.first),
        );
        expect(remote.stream, incoming);
        await media.offer();
        await media.receive(
          phone
              ? {'t': 'offer', 'sdp': 'remote'}
              : {
                  'type': 'video-offer',
                  'sdp': {'type': 'offer', 'sdp': 'remote'},
                },
        );
        await media.receive(
          phone
              ? {
                  't': 'ice',
                  'cand': {
                    'candidate': 'candidate',
                    'sdpMid': '0',
                    'sdpMLineIndex': 0,
                  },
                }
              : {
                  'type': 'video-ice',
                  'candidate': {
                    'candidate': 'candidate',
                    'sdpMid': '0',
                    'sdpMLineIndex': 0,
                  },
                },
        );
        pc.onIceCandidate!(RTCIceCandidate('local candidate', '0', 0));
        pc.onConnectionState!(
          RTCPeerConnectionState.RTCPeerConnectionStateConnected,
        );
        await Future<void>.delayed(Duration.zero);
        final signals = events
            .where((e) => e.type == 'signal')
            .map((e) => e.signal)
            .toList();
        expect(
          signals[0][phone ? 't' : 'type'],
          phone ? 'offer' : 'video-offer',
        );
        expect(
          signals[0]['sdp'],
          phone ? 'local offer' : {'sdp': 'local offer', 'type': 'offer'},
        );
        expect(
          signals[1][phone ? 't' : 'type'],
          phone ? 'answer' : 'video-answer',
        );
        expect(
          signals[2][phone ? 'cand' : 'candidate']['candidate'],
          'local candidate',
        );
        expect(events.any((e) => e.type == 'connected'), true);
        expect(pc.received, ['offer:remote', 'ice:candidate']);
        await media.mute(true);
        await media.camera(false);
        expect(audio.enabled, false);
        expect(video.enabled, false);
        audio.failStop = true;
        media.close();
        media.close();
        await media.released;
        expect(audio.stops, 1);
        expect(video.stops, 1);
        expect(incoming.tracks.single.stops, 1);
        expect(stream.disposals, 1);
        expect(incoming.disposals, 1);
        expect(local.disposals, 1);
        expect(remote.disposals, 1);
        expect(local.stream, isNull);
        expect(remote.stream, isNull);
        expect(pc.closes, 1);
        expect(pc.disposals, 1);
        expect(pc.onTrack, isNull);
        expect(pc.onIceCandidate, isNull);
      },
    );
  }
  testWidgets('microphone stats are sampled and stop after release', (
    tester,
  ) async {
    final pc = PeerFake();
    final media = WebRtcCallMedia(
      createPeer: (_) async => pc,
      acquire: (_) async => StreamFake([Track('audio')]),
    );
    final levels = <double>[];
    media.events.listen((e) {
      if (e.type == 'level') levels.add(e.signal!['value'] as double);
    });
    await media.open(const CallRef(peer, 'call0001'), null);
    pc.onConnectionState!(
      RTCPeerConnectionState.RTCPeerConnectionStateConnected,
    );
    await tester.pump(const Duration(milliseconds: 500));
    expect(levels, [0.4]);
    await media.mute(true);
    await tester.pump(const Duration(milliseconds: 500));
    expect(levels, [0.4, 0.0]);
    final lateState = pc.onConnectionState!;
    media.close();
    await media.released;
    lateState(RTCPeerConnectionState.RTCPeerConnectionStateConnected);
    await tester.pump(const Duration(seconds: 2));
    expect(levels, [0.4, 0.0]);
  });
  test(
    'video falls back to audio; missing devices negotiates receive only',
    () async {
      for (final noDevices in [false, true]) {
        final pc = PeerFake(), audio = Track('audio');
        var attempts = 0;
        final media = WebRtcCallMedia(
          createPeer: (_) async => pc,
          acquire: (constraints) async {
            attempts++;
            if (attempts == 1 || noDevices) throw StateError('no device');
            expect(constraints['video'], false);
            return StreamFake([audio]);
          },
          localRenderer: RendererFake(),
          remoteRenderer: RendererFake(),
        );
        final types = <String>[];
        media.events.listen((e) => types.add(e.type));
        await media.open(const CallRef(peer, 'call0001', video: true), null);
        await Future<void>.delayed(Duration.zero);
        expect(attempts, 2);
        expect(types, contains(noDevices ? 'recvonly' : 'audio-only'));
        expect(pc.directions, everyElement(TransceiverDirection.RecvOnly));
        expect(pc.directions.length, noDevices ? 2 : 1);
        media.close();
        await media.released;
      }
    },
  );
  test(
    'close during acquisition releases late tracks without adding them',
    () async {
      final pc = PeerFake(), late = Completer<MediaStream>();
      final entered = Completer<void>();
      final audio = Track('audio'), stream = StreamFake([]);
      stream.tracks.add(audio);
      final media = WebRtcCallMedia(
        createPeer: (_) async => pc,
        acquire: (_) {
          entered.complete();
          return late.future;
        },
      );
      final opening = media.open(const CallRef(peer, 'call0001'), null);
      await entered.future;
      media.close();
      late.complete(stream);
      await opening;
      await media.released;
      expect(pc.adds, 0);
      expect(audio.stops, 1);
      expect(stream.disposals, 1);
      expect(pc.closes, 1);
      expect(pc.disposals, 1);
    },
  );
  test(
    'close during peer creation and renderer initialization releases late resources',
    () async {
      final pc = PeerFake(), late = Completer<RTCPeerConnection>();
      final media = WebRtcCallMedia(
        createPeer: (_) => late.future,
        acquire: (_) async => throw StateError('must not acquire'),
      );
      final opening = media.open(const CallRef(peer, 'call0001'), null);
      media.close();
      late.complete(pc);
      await opening;
      await media.released;
      expect(pc.closes, 1);
      expect(pc.disposals, 1);
      final local = RendererFake()..initialization = Completer<void>(),
          remote = RendererFake();
      final second = WebRtcCallMedia(
        localRenderer: local,
        remoteRenderer: remote,
        createPeer: (_) async => throw StateError('must not create'),
      );
      final pending = second.open(
        const CallRef(peer, 'call0001', video: true),
        null,
      );
      second.close();
      local.initialization!.complete();
      await pending;
      await second.released;
      expect(local.disposals, 1);
      expect(remote.disposals, 0);
    },
  );
}
