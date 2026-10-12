import 'preferences.dart';
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'domain/call_media.dart';
import 'domain/call_coordinator.dart';
import 'domain/call_signaling.dart';
import 'domain/states.dart';
import 'transport/api.dart';

class CallsFeature extends StatefulWidget {
  final Api api;
  final bool Function() isActive;
  final VoidCallback onLocked;
  final void Function(ApiFailure) onFailure;
  final CallSignaling? signaling;
  final CallMedia Function()? mediaFactory;
  const CallsFeature({
    super.key,
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
    coordinator.poll();
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (widget.isActive()) coordinator.poll();
    });
  }

  void update() {
    if (mounted && widget.isActive()) setState(() {});
  }

  @override
  void dispose() {
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
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const UiText('Звонки', style: TextStyle(fontSize: 22)),
      const UiText('Входящие проверяются, пока открыт этот экран.'),
      UiText(label),
      if (coordinator.error != null)
        UiText(coordinator.error!, style: const TextStyle(color: Colors.red)),
      if (coordinator.call != null)
        SelectableText(
          '${coordinator.call!.name} · ${coordinator.call!.peer} · ${tr(context, coordinator.call!.video ? 'видео' : 'голос')}',
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
      if (coordinator.phase == CallPhase.idle) ...[
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
