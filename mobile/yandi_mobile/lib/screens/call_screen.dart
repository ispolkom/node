import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../services/call_service.dart';
import '../theme.dart';

/// Экран звонка: исходящий (идёт вызов), входящий (принять/отклонить) и разговор с видео или без.
class CallScreen extends StatefulWidget {
  const CallScreen({super.key});
  @override
  State<CallScreen> createState() => _CallScreenState();
}

class _CallScreenState extends State<CallScreen> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) { if (mounted) setState(() {}); });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final calls = context.read<AppState>().calls;
    return ListenableBuilder(
      listenable: calls,
      builder: (context, _) {
        // звонок закончился и сброшен: экран закрываем
        if (calls.phase == CallPhase.idle) {
          WidgetsBinding.instance.addPostFrameCallback((_) { if (mounted && Navigator.canPop(context)) Navigator.pop(context); });
        }
        return PopScope(
          canPop: false, // назад нельзя: завершить звонок можно кнопкой
          child: Scaffold(backgroundColor: Colors.black, body: SafeArea(child: _body(calls))),
        );
      },
    );
  }

  Widget _body(CallService c) {
    final showVideo = c.video && c.remoteRenderer?.srcObject != null && c.phase == CallPhase.active;
    return Stack(
      fit: StackFit.expand,
      children: [
        if (showVideo)
          RTCVideoView(c.remoteRenderer!, objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitCover)
        else
          Container(
            decoration: const BoxDecoration(
              gradient: LinearGradient(begin: Alignment.topCenter, end: Alignment.bottomCenter, colors: [Color(0xFF0B1B2B), Colors.black]),
            ),
          ),
        // имя и состояние
        Positioned(
          top: 24, left: 16, right: 16,
          child: Column(
            children: [
              if (!showVideo) ...[
                const SizedBox(height: 40),
                CircleAvatar(
                  radius: 54,
                  backgroundColor: AppTheme.surface,
                  child: Text(
                    c.peerName.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim().characters.firstOrNull?.toUpperCase() ?? '?',
                    style: const TextStyle(color: AppTheme.accent, fontSize: 44, fontWeight: FontWeight.bold),
                  ),
                ),
                const SizedBox(height: 16),
              ],
              Text(c.peerName, textAlign: TextAlign.center,
                  style: const TextStyle(color: Colors.white, fontSize: 24, fontWeight: FontWeight.w600)),
              const SizedBox(height: 6),
              Text(_status(c), textAlign: TextAlign.center,
                  style: const TextStyle(color: Colors.white70, fontSize: 15)),
              if (c.phase == CallPhase.active || c.phase == CallPhase.connecting)
                const Padding(
                  padding: EdgeInsets.only(top: 6),
                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                    Icon(Icons.lock, size: 13, color: Colors.white54),
                    SizedBox(width: 4),
                    Text('сквозное шифрование', style: TextStyle(color: Colors.white54, fontSize: 12)),
                  ]),
                ),
            ],
          ),
        ),
        // своё изображение
        if (c.video && c.localRenderer?.srcObject != null && c.phase != CallPhase.ringing)
          Positioned(
            right: 16, bottom: 150,
            width: 110, height: 150,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: c.cameraOn
                  ? RTCVideoView(c.localRenderer!, mirror: true, objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitCover)
                  : Container(color: Colors.black87, child: const Icon(Icons.videocam_off, color: Colors.white54)),
            ),
          ),
        Positioned(left: 0, right: 0, bottom: 28, child: _buttons(c)),
      ],
    );
  }

  String _status(CallService c) {
    switch (c.phase) {
      case CallPhase.calling:    return c.video ? 'Видеозвонок · вызываю…' : 'Вызываю…';
      case CallPhase.ringing:    return c.video ? 'Входящий видеозвонок' : 'Входящий звонок';
      case CallPhase.connecting: return 'Соединяю…';
      case CallPhase.active:     return CallService.formatDuration(c.elapsed);
      case CallPhase.ended:      return c.endReason;
      case CallPhase.idle:       return '';
    }
  }

  Widget _buttons(CallService c) {
    if (c.phase == CallPhase.ringing) {
      return Row(
        mainAxisAlignment: MainAxisAlignment.spaceEvenly,
        children: [
          _round(Icons.call_end, Colors.redAccent, 'Отклонить', c.reject),
          _round(c.video ? Icons.videocam : Icons.call, Colors.green, 'Принять', c.accept),
        ],
      );
    }
    if (c.phase == CallPhase.ended) return const SizedBox.shrink();
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceEvenly,
      children: [
        _small(c.muted ? Icons.mic_off : Icons.mic, c.muted, 'Микрофон', c.toggleMute),
        _small(c.speaker ? Icons.volume_up : Icons.hearing, c.speaker, c.speaker ? 'Динамик' : 'Разговорный', c.toggleSpeaker),
        if (c.video) _small(c.cameraOn ? Icons.videocam : Icons.videocam_off, !c.cameraOn, 'Камера', c.toggleCamera),
        if (c.video) _small(Icons.cameraswitch, false, 'Повернуть', c.switchCamera),
        _round(Icons.call_end, Colors.redAccent, 'Завершить', c.hangup),
      ],
    );
  }

  Widget _round(IconData icon, Color color, String label, VoidCallback onTap) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      GestureDetector(
        onTap: onTap,
        child: Container(width: 68, height: 68, decoration: BoxDecoration(shape: BoxShape.circle, color: color),
            child: Icon(icon, color: Colors.white, size: 32)),
      ),
      const SizedBox(height: 6),
      Text(label, style: const TextStyle(color: Colors.white70, fontSize: 12)),
    ],
  );

  Widget _small(IconData icon, bool on, String label, VoidCallback onTap) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      GestureDetector(
        onTap: onTap,
        child: Container(
          width: 52, height: 52,
          decoration: BoxDecoration(shape: BoxShape.circle, color: on ? Colors.white : Colors.white24),
          child: Icon(icon, color: on ? Colors.black : Colors.white, size: 24),
        ),
      ),
      const SizedBox(height: 6),
      Text(label, style: const TextStyle(color: Colors.white70, fontSize: 11)),
    ],
  );
}
