import 'dart:async';
import 'package:flutter/material.dart';
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
      mediaFactory: widget.mediaFactory,
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
    child: Text(title),
  );
  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const Text('Звонки', style: TextStyle(fontSize: 22)),
      const Text(
        'Сигнальный режим: реальное аудио и видео недоступны до подключения нативной библиотеки WebRTC. При принятии звонка медиа-слой сообщит об отсутствии библиотеки. Входящие проверяются, пока открыт этот экран.',
      ),
      Text(label),
      if (coordinator.error != null)
        Text(coordinator.error!, style: const TextStyle(color: Colors.red)),
      if (coordinator.call != null)
        SelectableText(
          '${coordinator.call!.name} · ${coordinator.call!.peer} · ${coordinator.call!.video ? 'видео' : 'голос'}',
        ),
      if (coordinator.phase == CallPhase.idle) ...[
        TextField(
          controller: peer,
          decoration: InputDecoration(
            labelText: phone
                ? 'ID телефона (64 hex)'
                : 'ID собеседника (16 или 64 hex)',
          ),
        ),
        CheckboxListTile(
          value: phone,
          onChanged: (v) => setState(() => phone = v!),
          title: const Text('Телефон'),
        ),
        CheckboxListTile(
          value: video,
          onChanged: (v) => setState(() => video = v!),
          title: const Text('Видео'),
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
        Text(
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
              ),
          ],
        ),
      ],
    ],
  );
}
