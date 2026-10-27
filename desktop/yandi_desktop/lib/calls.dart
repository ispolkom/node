import 'theme.dart';
import 'preferences.dart';
import 'platform_files.dart';
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
  final nameNotifier = ValueNotifier<String>('');
  Route<void>? overlay;
  AppPreferences? _preferences;
  String _ringKind = 'none';

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final next = PreferencesScope.of(context);
    if (_preferences != next) {
      _preferences?.removeListener(syncRingtone);
      _preferences = next;
      _preferences?.addListener(syncRingtone);
    }
    syncRingtone();
  }

  void syncRingtone() {
    final enabled = _preferences?.sound == true && _preferences?.dnd != true;
    final kind = !enabled ? 'none' : switch (coordinator.phase) {
      CallPhase.incomingRinging => coordinator.call?.video == true ? 'video' : 'incoming',
      CallPhase.outgoingRinging => 'outgoing',
      _ => 'none',
    };
    if (kind == _ringKind) return;
    _ringKind = kind;
    unawaited(PlatformFiles.channel.invokeMethod<void>('ringtone', {'kind': kind}).catchError((Object _) {}));
  }

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
            nameNotifier.value = resolvedName;
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
    syncRingtone();
    final c = coordinator.call;
    if (c != null && resolvedPeer != c.peer) {
      resolvedPeer = c.peer;
      resolvedName = cleanName(c.name);
      nameNotifier.value = resolvedName;
      resolveName(c.peer);
    }
    if (mounted && widget.isActive()) setState(() {});
    WidgetsBinding.instance.addPostFrameCallback((_) => syncOverlay());
  }

  /// Идёт звонок — экран звонка поверх всего окна (как в Skype); закончился — закрывается сам.
  void syncOverlay() {
    if (!widget.embedded || !mounted) return;
    final inCall = coordinator.phase != CallPhase.idle;
    if (inCall && overlay == null) {
      final route = RawDialogRoute<void>(
        barrierDismissible: false,
        barrierColor: const Color(0xE60E2A12),
        pageBuilder: (ctx, _, _) => PopScope(canPop: false, child: Center(child: callCard(ctx))),
      );
      overlay = route;
      Navigator.of(context).push(route);
    } else if (!inCall && overlay != null) {
      closeOverlay();
    }
  }

  void closeOverlay() {
    final r = overlay;
    overlay = null;
    if (r != null && r.isActive) r.navigator?.removeRoute(r);
  }

  @override
  void dispose() {
    _preferences?.removeListener(syncRingtone);
    if (_ringKind != 'none') {
      unawaited(PlatformFiles.channel.invokeMethod<void>('ringtone', {'kind': 'none'}).catchError((Object _) {}));
    }
    widget.controller?._start = null;
    closeOverlay();
    nameNotifier.dispose();
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
  String get clock =>
      '${coordinator.seconds ~/ 60}:${(coordinator.seconds % 60).toString().padLeft(2, '0')}';

  Widget round(IconData icon, String tip, VoidCallback? onTap, {Color color = const Color(0x33FFFFFF), bool big = true}) => Tooltip(
    message: tr(context, tip),
    waitDuration: const Duration(seconds: 2),
    child: Material(
      color: onTap == null ? color.withValues(alpha: 0.4) : color,
      shape: const CircleBorder(),
      child: InkWell(
        customBorder: const CircleBorder(),
        onTap: onTap,
        child: Padding(
          padding: EdgeInsets.all(big ? 18 : 14),
          child: Icon(icon, size: big ? 30 : 26, color: Colors.white),
        ),
      ),
    ),
  );

  /// Экран звонка: имя, состояние, видео, значки «принять/отклонить/микрофон/камера/завершить».
  Widget callCard(BuildContext ctx) => ListenableBuilder(
    listenable: Listenable.merge([coordinator, nameNotifier]),
    builder: (_, _) {
      final c = coordinator.call;
      final video = c?.video == true;
      final name = nameNotifier.value;
      final media = coordinator.media;
      final phase = coordinator.phase;
      final status = phase == CallPhase.active ? clock : label;
      return Material(
        color: Colors.transparent,
        child: Container(
          key: const ValueKey('call-screen'),
          constraints: BoxConstraints(maxWidth: video && phase == CallPhase.active ? 820 : 440),
          margin: const EdgeInsets.all(16),
          padding: const EdgeInsets.all(28),
          decoration: BoxDecoration(
            color: const Color(0xFF1E5226),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: Colors.white24),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Image.asset('assets/logo.png', width: 24, height: 24),
                  const SizedBox(width: 8),
                  const Text('YANDI', style: TextStyle(fontSize: 16, fontWeight: FontWeight.bold, letterSpacing: 2, color: Colors.white70)),
                ],
              ),
              const SizedBox(height: 18),
              if (media is WebRtcCallMedia && video && phase == CallPhase.active)
                Stack(
                  children: [
                    AspectRatio(aspectRatio: 16 / 9, child: ClipRRect(borderRadius: BorderRadius.circular(8), child: RTCVideoView(media.remoteRenderer))),
                    Positioned(
                      right: 10,
                      bottom: 10,
                      child: SizedBox(width: 160, height: 120, child: ClipRRect(borderRadius: BorderRadius.circular(6), child: RTCVideoView(media.localRenderer, mirror: true))),
                    ),
                  ],
                )
              else
                CircleAvatar(
                  radius: 52,
                  backgroundColor: Colors.white24,
                  child: name.isEmpty
                      ? const Icon(Icons.person, size: 60, color: Colors.white)
                      : Text(String.fromCharCodes(name.runes.take(1)).toUpperCase(), style: const TextStyle(fontSize: 44, color: Colors.white)),
                ),
              const SizedBox(height: 14),
              if (name.isNotEmpty) Text(name, style: const TextStyle(fontSize: 24, color: Colors.white, fontWeight: FontWeight.bold)),
              const SizedBox(height: 4),
              UiText(video ? 'Видеозвонок' : 'Голосовой звонок', style: const TextStyle(color: Colors.white70)),
              const SizedBox(height: 4),
              UiText(status, key: const ValueKey('call-status'), style: const TextStyle(fontSize: 18, color: Colors.white)),
              if (coordinator.error != null) ...[
                const SizedBox(height: 8),
                UiText(coordinator.error!, style: const TextStyle(color: Color(0xFFFFB4AB))),
              ],
              if (coordinator.mediaNote != null) ...[
                const SizedBox(height: 8),
                UiText(coordinator.mediaNote!, style: const TextStyle(color: Colors.white70)),
              ],
              if (phase == CallPhase.active && coordinator.microphoneLevel != null) ...[
                const SizedBox(height: 10),
                SizedBox(width: 200, child: LinearProgressIndicator(value: coordinator.microphoneLevel, color: Colors.white70, backgroundColor: Colors.white12)),
              ],
              const SizedBox(height: 22),
              Wrap(
                alignment: WrapAlignment.center,
                spacing: 18,
                runSpacing: 12,
                children: [
                  if (phase == CallPhase.incomingRinging) ...[
                    round(Icons.call, 'Принять', coordinator.busy ? null : () => coordinator.accept(), color: const Color(0xFF2E9E3F)),
                    round(Icons.call_end, 'Отклонить', () => coordinator.end(reject: true), color: const Color(0xFFD32F2F)),
                  ] else if (coordinator.running) ...[
                    if (phase == CallPhase.active) ...[
                      round(
                        coordinator.muted ? Icons.mic_off : Icons.mic,
                        coordinator.muted ? 'Включить микрофон' : 'Выключить микрофон',
                        coordinator.busy ? null : () => coordinator.mute(),
                        color: coordinator.muted ? const Color(0xFFB26A00) : const Color(0x33FFFFFF),
                      ),
                      if (video)
                        round(
                          coordinator.cameraEnabled ? Icons.videocam : Icons.videocam_off,
                          coordinator.cameraEnabled ? 'Выключить камеру' : 'Включить камеру',
                          coordinator.busy || coordinator.mediaNote != null ? null : () => coordinator.camera(),
                          color: coordinator.cameraEnabled ? const Color(0x33FFFFFF) : const Color(0xFFB26A00),
                        ),
                    ],
                    round(
                      Icons.call_end,
                      'Завершить',
                      phase == CallPhase.ending ? null : () => coordinator.end(),
                      color: const Color(0xFFD32F2F),
                    ),
                  ],
                ],
              ),
            ],
          ),
        ),
      );
    },
  );

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
          (coordinator.phase != CallPhase.idle || coordinator.error == null)
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
