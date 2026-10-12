import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:video_player/video_player.dart';
import '../theme.dart';

/// Видео как на YouTube: тап по картинке показывает/скрывает управление, по центру белый треугольник «играть» (на паузе) или пауза,
/// внизу время, полоса перемотки и значок «на весь экран» в правом нижнем углу. Во время воспроизведения управление скрывается само
/// через несколько секунд. Сам кадр ([video]) и состояние ([value]) приходят снаружи, поэтому виджет не зависит от платформенного плеера.
class VideoSurface extends StatefulWidget {
  final Widget video;
  final ValueListenable<VideoPlayerValue> value;
  final VoidCallback onPlay, onPause, onToggleFullscreen;
  final void Function(Duration) onSeek;
  final bool fullscreen;
  /// Через сколько скрывать управление во время воспроизведения.
  final Duration hideAfter;
  const VideoSurface({
    super.key,
    required this.video,
    required this.value,
    required this.onPlay,
    required this.onPause,
    required this.onSeek,
    required this.onToggleFullscreen,
    this.fullscreen = false,
    this.hideAfter = const Duration(seconds: 4),
  });

  @override
  State<VideoSurface> createState() => _VideoSurfaceState();
}

class _VideoSurfaceState extends State<VideoSurface> {
  bool _visible = true;
  Timer? _hide;
  bool _wasPlaying = false;

  @override
  void initState() {
    super.initState();
    widget.value.addListener(_onValue);
  }

  @override
  void dispose() {
    widget.value.removeListener(_onValue);
    _hide?.cancel();
    super.dispose();
  }

  void _onValue() {
    final playing = widget.value.value.isPlaying;
    if (playing != _wasPlaying) {
      _wasPlaying = playing;
      if (playing) {
        _armHide();
      } else {
        _hide?.cancel();
        if (mounted) setState(() => _visible = true);
      }
    }
    if (mounted) setState(() {});
  }

  void _armHide() {
    _hide?.cancel();
    _hide = Timer(widget.hideAfter, () {
      if (mounted && widget.value.value.isPlaying) setState(() => _visible = false);
    });
  }

  void _tap() {
    final v = widget.value.value;
    if (!v.isPlaying) {
      // на паузе тап по видео — играть, как на YouTube
      if (_visible || v.position == Duration.zero) {
        widget.onPlay();
      }
      setState(() => _visible = true);
      return;
    }
    if (_visible) {
      widget.onPause();
      setState(() => _visible = true);
    } else {
      setState(() => _visible = true);
      _armHide();
    }
  }

  static String _mmss(Duration d) {
    final h = d.inHours;
    final m = (d.inMinutes % 60).toString().padLeft(2, '0');
    final s = (d.inSeconds % 60).toString().padLeft(2, '0');
    return h > 0 ? '$h:$m:$s' : '${d.inMinutes}:$s';
  }

  @override
  Widget build(BuildContext context) {
    final v = widget.value.value;
    final dur = v.duration.inMilliseconds;
    final pos = v.position.inMilliseconds.clamp(0, dur == 0 ? 1 : dur).toDouble();
    final showControls = _visible || !v.isPlaying;
    return Container(
      color: Colors.black,
      child: Stack(fit: StackFit.expand, children: [
        Center(child: widget.video),
        GestureDetector(
          key: const ValueKey('video-tap'),
          behavior: HitTestBehavior.opaque,
          onTap: _tap,
          child: const SizedBox.expand(),
        ),
        if (showControls) ...[
          IgnorePointer(
            child: Center(
              child: Container(
                width: 72,
                height: 72,
                decoration: const BoxDecoration(color: Color(0x66000000), shape: BoxShape.circle),
                child: Icon(
                  v.isPlaying ? Icons.pause : Icons.play_arrow,
                  key: ValueKey(v.isPlaying ? 'video-pause' : 'video-play'),
                  color: Colors.white,
                  size: 52,
                ),
              ),
            ),
          ),
        ],
        // Полоса перемотки видна всегда, в том числе когда управление скрыто: можно тронуть и мотнуть, например, на середину.
        Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 0, 4, 0),
            decoration: BoxDecoration(
              gradient: showControls
                  ? const LinearGradient(
                      begin: Alignment.bottomCenter,
                      end: Alignment.topCenter,
                      colors: [Color(0xAA000000), Color(0x00000000)],
                    )
                  : null,
            ),
            child: Column(mainAxisSize: MainAxisSize.min, children: [
              if (showControls)
                Row(children: [
                  Text(_mmss(v.position), style: const TextStyle(color: Colors.white, fontSize: 12)),
                  const SizedBox(width: 6),
                  Text('/ ${_mmss(v.duration)}', style: const TextStyle(color: Colors.white70, fontSize: 12)),
                  const Spacer(),
                  IconButton(
                    key: const ValueKey('video-fullscreen'),
                    tooltip: widget.fullscreen ? 'Свернуть' : 'На весь экран',
                    icon: Icon(widget.fullscreen ? Icons.fullscreen_exit : Icons.fullscreen, color: Colors.white),
                    onPressed: () {
                      widget.onToggleFullscreen();
                      if (widget.value.value.isPlaying) _armHide();
                    },
                  ),
                ]),
              SizedBox(
                height: 30,
                child: SliderTheme(
                  data: SliderTheme.of(context).copyWith(
                    trackHeight: showControls ? 3.5 : 3,
                    thumbShape: RoundSliderThumbShape(enabledThumbRadius: showControls ? 7 : 4),
                    overlayShape: const RoundSliderOverlayShape(overlayRadius: 14),
                    activeTrackColor: AppTheme.accent,
                    thumbColor: AppTheme.accent,
                    inactiveTrackColor: Colors.white38,
                  ),
                  child: Slider(
                    key: const ValueKey('video-seek'),
                    value: pos,
                    max: dur == 0 ? 1 : dur.toDouble(),
                    onChanged: dur == 0 ? null : (x) => widget.onSeek(Duration(milliseconds: x.round())),
                    onChangeStart: (_) {
                      _hide?.cancel();
                      if (!_visible) setState(() => _visible = true);
                    },
                    onChangeEnd: (_) {
                      if (widget.value.value.isPlaying) _armHide();
                    },
                  ),
                ),
              ),
            ]),
          ),
        ),
      ]),
    );
  }
}
