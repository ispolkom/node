// This probe is only invoked inside the chaos fixture's private /tmp namespace.
import 'dart:async';
import '../lib/transport/api.dart';
import '../lib/domain/call_signaling.dart';

Future<CallRef> incoming(DaemonCallSignaling s, String id) async {
  final deadline = DateTime.now().add(const Duration(seconds: 30));
  while (DateTime.now().isBefore(deadline)) {
    final rows = await s.poll();
    for (final r in rows) {
      if (r.id == id) return r;
    }
    await Future<void>.delayed(const Duration(milliseconds: 100));
  }
  throw StateError('incoming signaling deadline');
}

Future<CallSignal> event(DaemonCallSignaling s, String id, String type) => s
    .signals
    .firstWhere((e) => e.data['call_id'] == id && e.data['type'] == type)
    .timeout(const Duration(seconds: 30));
Future<void> main(List<String> args) async {
  final a = UnixApi(args[0]), b = UnixApi(args[1]);
  final sa = DaemonCallSignaling(a), sb = DaemonCallSignaling(b);
  try {
    final peer = (await b.request('/api/status'))['short_id'] as String;
    for (final video in [false, true]) {
      final c = await sa.start(peer, video, false);
      await sa.watch(c);
      final r = await incoming(sb, c.id);
      if (r.video != video) throw StateError('call family');
      await sb.watch(r);
      final accepted = event(sa, c.id, 'call-accept');
      await sb.control(r, 'accept');
      await accepted;
      final offer = event(sb, c.id, video ? 'video-offer' : 'offer');
      await sa.send(c, {
        'type': video ? 'video-offer' : 'offer',
        'sdp': {'type': 'offer', 'sdp': 'fixture-signaling-only'},
      });
      await offer;
      final ended = event(sa, c.id, 'hangup');
      await sb.control(r, 'hangup');
      await ended;
      sa.release();
      sb.release();
      final rejectedCall = await sa.start(peer, video, false);
      await sa.watch(rejectedCall);
      final rejectedIncoming = await incoming(sb, rejectedCall.id);
      final rejected = event(sa, rejectedCall.id, 'call-reject');
      await sb.control(rejectedIncoming, 'reject');
      await rejected;
      sa.release();
      sb.release();
    }
    await sa.turn();
    print(
      'PASS: node voice/video incoming, accept/reject/end, SDP forwarding over Unix WS; no media',
    );
  } finally {
    sa.close();
    sb.close();
    a.cancel();
    b.cancel();
  }
}
