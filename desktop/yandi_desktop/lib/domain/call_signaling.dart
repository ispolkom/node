import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import '../transport/api.dart';

class CallRef {
  final String peer, id, name;
  final bool video, phone, incoming;
  const CallRef(
    this.peer,
    this.id, {
    this.name = '',
    this.video = false,
    this.phone = false,
    this.incoming = false,
  });
}

class CallSignal {
  final String peer;
  final Map<String, dynamic> data;
  const CallSignal(this.peer, this.data);
}

abstract class CallSignaling {
  Stream<CallSignal> get signals;
  Future<CallRef> start(String peer, bool video, bool phone);
  Future<void> watch(CallRef call);
  Future<List<CallRef>> poll();
  Future<void> control(CallRef call, String type);
  Future<void> send(CallRef call, Map<String, dynamic> data);
  Future<Map<String, dynamic>?> turn();
  Future<void> log(CallRef call, String text);
  void release();
  void close();
}

/// Carries opaque SDP/ICE only in memory. Daemon owns E4 and credential storage.
class DaemonCallSignaling implements CallSignaling {
  final Api api;
  final Future<WebSocket> Function(String)? socketFactory;
  final DateTime Function() now;
  final _events = StreamController<CallSignal>.broadcast();
  WebSocket? socket;
  StreamSubscription<dynamic>? subscription;
  int generation = 0, after = 0;
  bool baseline = false, closed = false;
  DaemonCallSignaling(this.api, {this.socketFactory, DateTime Function()? now})
    : now = now ?? DateTime.now;
  @override
  Stream<CallSignal> get signals => _events.stream;
  String family(CallRef c) => '/api/media/${c.video ? 'video/' : ''}';
  String peer(String value, {bool phone = false}) {
    if (!RegExp(r'^[a-fA-F0-9]{16}([a-fA-F0-9]{48})?$').hasMatch(value))
      throw const FormatException('ID: 16 или 64 шестнадцатеричных символа');
    if (phone && value.length != 64)
      throw const FormatException('ID телефона: 64 шестнадцатеричных символа');
    return value.toLowerCase();
  }

  String id(String value, {bool phone = false}) {
    if (!RegExp(r'^[a-zA-Z0-9_-]{1,64}$').hasMatch(value))
      throw const ApiFailure('protocol');
    if (phone && value.length < 8) throw const ApiFailure('protocol');
    return value;
  }

  @override
  Future<CallRef> start(String target, bool video, bool phone) async {
    target = peer(target, phone: phone);
    if (phone) {
      final cid = List.generate(
        16,
        (_) => Random.secure().nextInt(256),
      ).map((v) => v.toRadixString(16).padLeft(2, '0')).join();
      final c = CallRef(target, cid, phone: true, video: video);
      await control(c, 'invite');
      return c;
    }
    final c = CallRef(target, '', video: video);
    final result = await api.request(
      '${family(c)}call/start',
      body: {'peer_id': target, 'audio_enabled': true, 'video_enabled': video},
    );
    return CallRef(target, id(result['call_id'] as String), video: video);
  }

  @override
  Future<void> watch(CallRef call) async {
    final own = ++generation;
    await subscription?.cancel();
    socket?.close();
    socket = null;
    if (closed || call.phone) return;
    final factory =
        socketFactory ??
        (api is UnixApi ? (api as UnixApi).openMediaSocket : null);
    if (factory == null) throw const ApiFailure('protocol');
    final next = await factory(call.peer);
    if (closed || own != generation) {
      await next.close();
      return;
    }
    socket = next;
    subscription = next.listen(
      (raw) {
        if (closed ||
            own != generation ||
            raw is! String ||
            raw.length > 262144)
          return;
        try {
          final data = jsonDecode(raw);
          if (data is Map<String, dynamic>)
            _events.add(CallSignal(call.peer, data));
        } catch (_) {
          /* Reject malformed signaling without logging payload. */
        }
      },
      onError: (Object _) {
        if (!closed && own == generation)
          _events.add(
            CallSignal(call.peer, {
              'type': 'transport-lost',
              'call_id': call.id,
            }),
          );
      },
      onDone: () {
        if (!closed && own == generation)
          _events.add(
            CallSignal(call.peer, {
              'type': 'transport-lost',
              'call_id': call.id,
            }),
          );
      },
    );
  }

  @override
  Future<List<CallRef>> poll() async {
    final refs = <CallRef>[];
    for (final video in [false, true]) {
      final r = await api.request(
        '/api/media/${video ? 'video/' : ''}incoming-call',
      );
      if (closed) return [];
      if (r['call_id'] is String)
        refs.add(
          CallRef(
            peer(r['from_short_id'] as String),
            id(r['call_id'] as String),
            name: r['from_display_name'] as String? ?? '',
            video: video,
            incoming: true,
          ),
        );
    }
    final result = await api.request('/api/mobile/call/events?after=$after');
    if (closed) return [];
    final last = result['last'];
    if (last is! int || result['events'] is! List)
      throw const ApiFailure('protocol');
    if (!baseline) {
      after = last;
      baseline = true;
      return refs;
    }
    final events =
        (result['events'] as List).whereType<Map<String, dynamic>>().toList()
          ..sort((a, b) => (a['seq'] as int).compareTo(b['seq'] as int));
    final cancelled = <String>{};
    for (final event in events) {
      final seq = event['seq'];
      if (seq is! int || seq <= after) continue;
      after = seq;
      final text = event['text'];
      if (text is! String ||
          !text.startsWith('\u0001yandi-call:') ||
          text.length > 262144)
        continue;
      try {
        final data = jsonDecode(text.substring(12));
        if (data is! Map<String, dynamic> ||
            data['v'] != 1 ||
            data['video'] is! bool ||
            data['ts'] is! int ||
            (now().millisecondsSinceEpoch - (data['ts'] as int)).abs() > 90000)
          continue;
        final sender = peer(event['from'] as String, phone: true);
        id(data['cid'] as String, phone: true);
        if (const ['hangup', 'reject', 'busy'].contains(data['t'])) {
          cancelled.add('$sender/${data['cid']}');
        }
        if (data['t'] == 'invite')
          refs.add(
            CallRef(
              sender,
              data['cid'],
              name: event['name'] as String? ?? '',
              video: data['video'],
              phone: true,
              incoming: true,
            ),
          );
        else
          _events.add(CallSignal(sender, data));
      } catch (_) {
        /* Untrusted phone payload is never logged. */
      }
    }
    if (last > after) after = last;
    refs.removeWhere((c) => cancelled.contains('${c.peer}/${c.id}'));
    return refs;
  }

  @override
  Future<void> control(CallRef c, String type) async {
    if (c.phone) {
      await send(c, {'t': type});
      return;
    }
    if (type == 'accept' || type == 'reject' || type == 'busy') {
      await api.request(
        '${family(c)}call/${id(c.id)}/${type == 'accept' ? 'accept' : 'reject'}',
        method: 'POST',
      );
    } else if (type == 'hangup') {
      await api.request(
        '${family(c)}call/end',
        body: {'peer_id': c.peer, 'call_id': c.id},
      );
    }
  }

  @override
  Future<void> send(CallRef c, Map<String, dynamic> data) async {
    if (c.phone) {
      await api.request(
        '/api/mobile/call/signal',
        body: {
          'peer': c.peer,
          'text':
              '\u0001yandi-call:${jsonEncode({...data, 'v': 1, 'cid': c.id, 'video': c.video, 'ts': now().millisecondsSinceEpoch})}',
        },
      );
    } else {
      if (socket == null || socket!.readyState != WebSocket.open)
        throw const ApiFailure('disconnected');
      socket!.add(
        jsonEncode({
          ...data,
          'call_id': c.id,
          if (c.video) 'call_type': 'video',
        }),
      );
    }
  }

  @override
  Future<void> log(CallRef c, String text) async {
    if (!c.phone) return;
    if (!text.startsWith('📞') ||
        text.runes.length > 200 ||
        RegExp(r'[\x00-\x1f\x7f]').hasMatch(text))
      throw const ApiFailure('protocol');
    await api.request(
      '/api/mobile/call/log',
      body: {'peer': c.peer, 'outgoing': !c.incoming, 'text': text},
    );
  }

  @override
  Future<Map<String, dynamic>?> turn() async {
    final r = await api.request('/api/mobile/call/turn');
    return r['turn'] as Map<String, dynamic>?;
  }

  @override
  void release() {
    generation++;
    subscription?.cancel();
    socket?.close();
    socket = null;
  }

  @override
  void close() {
    closed = true;
    release();
    _events.close();
  }
}
