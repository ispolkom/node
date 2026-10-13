import 'theme.dart';
import 'preferences.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'domain/call_media.dart';
import 'domain/call_coordinator.dart';
import 'domain/call_signaling.dart';
import 'domain/states.dart';
import 'transport/api.dart';

/// Lets the chat start a call through the app-wide call panel.
class CallsController {
  Future<void> Function(String peer, bool video, bool phone)? _start;
  bool get ready => _start != null;
  Future<void> start(String peer, {bool video = false, bool phone = false}) =>
      _start?.call(peer, video, phone) ?? Future.value();
}

class CallsFeature extends StatefulWidget {
  /// Embedded: lives under the header on every page, shows nothing while idle.
  final bool embedded;
  final CallsController? controller;
  final Api api;
  final bool Function() isActive;
  final VoidCallback onLocked;
  final void Function(ApiFailure) onFailure;
  final CallSignaling? signaling;
  final CallMedia Function()? mediaFactory;
  const CallsFeature({
    super.key,
    this.embedded = false,
    this.controller,
    required this.api,
    required this.isActive,
    required this.onLocked,
    required this.onFailure,
    this.signaling,
    this.mediaFactory,
  });
  @override
  State<CallsFeature> createState() => _CallsFeatureState();
}

class _CallsFeatureState extends State<CallsFeature> {
  final peer = TextEditingController();
  late final CallCoordinator coordinator;
  Timer? timer;
  bool video = false, phone = false;
  /// Имя собеседника из моих контактов (то, что я ему дал), без значка устройства: кто именно отвечает, не важно.
  String resolvedPeer = '', resolvedName = '';

  static String cleanName(String name) => name
      .replaceFirst(RegExp('^[\\s\u{1F4F1}\u{1F4BB}\u{1F5A5}\uFE0F]+', unicode: true), '')
      .trim();

  Future<void> resolveName(String peerId) async {
    try {
      final r = await widget.api.request('/api/contacts');
      final list = r['contacts'];
      if (list is! List) return;
      for (final c in list) {
        if (c is! Map) continue;
        final sid = '${c['short_id'] ?? ''}';
        if (sid.isNotEmpty && (peerId.startsWith(sid) || sid.startsWith(peerId))) {
          if (mounted && coordinator.call?.peer == peerId) {
            setState(() => resolvedName = cleanName('${c['name'] ?? ''}'));
          }
          return;
        }
      }
    } catch (_) {
      /* имя необязательно: без него показывается только логотип */
    }
  }
  @override
  void initState() {
    super.initState();
    coordinator = CallCoordinator(
      signaling: widget.signaling ?? DaemonCallSignaling(widget.api),
      mediaFactory: widget.mediaFactory ?? WebRtcCallMedia.new,
      authorize: () async {
        if (!mounted || !widget.isActive()) return false;
        final vault = VaultState.fromJson(
          await widget.api.request('/api/vault/status'),
        );
        if (!mounted || !widget.isActive()) return false;
        if (vault.hasError || vault.configured && !vault.unlocked) {
          widget.onLocked();
          return false;
        }
        return true;
      },
    );
    coordinator.addListener(update);
    widget.controller?._start = (peer, video, phone) async {
      if (coordinator.phase != CallPhase.idle || coordinator.busy) return;
      await coordinator.start(peer, video: video, phone: phone);
    };
    coordinator.poll();
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (widget.isActive()) coordinator.poll();
    });
  }

  void update() {
    final c = coordinator.call;
    if (c != null && resolvedPeer != c.peer) {
      resolvedPeer = c.peer;
      resolvedName = cleanName(c.name);
      resolveName(c.peer);
    }
    if (mounted && widget.isActive()) setState(() {});
  }

  @override
  void dispose() {
    widget.controller?._start = null;
    timer?.cancel();
    coordinator.removeListener(update);
    coordinator.dispose();
    peer.clear();
    peer.dispose();
    super.dispose();
  }

  String get label => switch (coordinator.phase) {
    CallPhase.idle => 'Ожидание звонка',
    CallPhase.incomingRinging => 'Входящий звонок',
    CallPhase.outgoingRinging => 'Исходящий звонок',
    CallPhase.connecting => 'Соединение',
    CallPhase.active => 'Разговор',
    CallPhase.ending => 'Завершение',
    CallPhase.ended => 'Звонок завершён',
    CallPhase.error => 'Ошибка звонка',
  };
  Widget button(
    String title,
    VoidCallback action, {
    bool enabled = true,
    bool allowBusy = false,
  }) => OutlinedButton(
    onPressed: enabled && (allowBusy || !coordinator.busy) ? action : null,
    child: UiText(title),
  );
  @override
  Widget build(BuildContext context) => widget.embedded &&
          coordinator.phase == CallPhase.idle &&
          coordinator.error == null
      ? const SizedBox.shrink()
      : PanelStack(
    children: [
      if (!widget.embedded) ...[
        const UiText('Звонки', style: TextStyle(fontSize: 22)),
        const UiText('Входящие проверяются, пока открыт этот экран.'),
      ],
      UiText(label),
      if (coordinator.error != null)
        NoticePanel(coordinator.error!, error: true),
      if (coordinator.call != null)
        Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // звонок в сети YANDI: логотип и название над именем, чтобы не спутать с другим мессенджером или сотовой связью
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Image.asset('assets/logo.png', width: 30, height: 30),
                const SizedBox(width: 8),
                const Text(
                  'YANDI',
                  style: TextStyle(
                    fontSize: 20,
                    fontWeight: FontWeight.bold,
                    letterSpacing: 2,
                    color: Icq.header,
                  ),
                ),
              ],
            ),
            if (resolvedName.isNotEmpty)
              Text(resolvedName, style: const TextStyle(fontSize: 18)),
            UiText(coordinator.call!.video ? 'Видеозвонок' : 'Голосовой звонок'),
          ],
        ),
      if (coordinator.microphoneLevel != null) ...[
        const UiText('Уровень микрофона'),
        LinearProgressIndicator(value: coordinator.microphoneLevel),
      ],
      if (coordinator.mediaNote != null) UiText(coordinator.mediaNote!),
      if (coordinator.media case final WebRtcCallMedia media)
        if (coordinator.call?.video == true)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Column(
              children: [
                AspectRatio(
                  aspectRatio: 16 / 9,
                  child: RTCVideoView(media.remoteRenderer),
                ),
                const UiText('Собеседник'),
                SizedBox(
                  width: 160,
                  height: 120,
                  child: RTCVideoView(media.localRenderer, mirror: true),
                ),
                const UiText('Вы'),
              ],
            ),
          ),
      if (coordinator.phase == CallPhase.idle && !widget.embedded) ...[
        TextField(
          controller: peer,
          decoration: InputDecoration(
            labelText: tr(
              context,
              phone ? 'ID телефона (64 hex)' : 'ID собеседника (16 или 64 hex)',
            ),
          ),
        ),
        CheckboxListTile(
          value: phone,
          onChanged: (v) => setState(() => phone = v!),
          title: const UiText('Телефон'),
        ),
        CheckboxListTile(
          value: video,
          onChanged: (v) => setState(() => video = v!),
          title: const UiText('Видео'),
        ),
        button(
          'Позвонить',
          () => coordinator.start(peer.text.trim(), video: video, phone: phone),
        ),
      ],
      if (coordinator.phase == CallPhase.incomingRinging)
        Wrap(
          spacing: 8,
          children: [
            button('Принять', () => coordinator.accept()),
            button('Отклонить', () => coordinator.end(reject: true)),
          ],
        ),
      if (coordinator.running)
        button(
          'Завершить',
          () => coordinator.end(),
          enabled: coordinator.phase != CallPhase.ending,
          allowBusy: true,
        ),
      if (coordinator.phase == CallPhase.active) ...[
        UiText(
          'Длительность: ${coordinator.seconds ~/ 60}:${(coordinator.seconds % 60).toString().padLeft(2, '0')}',
        ),
        Wrap(
          spacing: 8,
          children: [
            button(
              coordinator.muted ? 'Включить микрофон' : 'Выключить микрофон',
              () => coordinator.mute(),
            ),
            if (coordinator.call!.video)
              button(
                coordinator.cameraEnabled
                    ? 'Выключить камеру'
                    : 'Включить камеру',
                () => coordinator.camera(),
                enabled: coordinator.mediaNote == null,
              ),
          ],
        ),
      ],
    ],
  );
}
