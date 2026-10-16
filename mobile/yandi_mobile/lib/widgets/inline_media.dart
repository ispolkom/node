import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/material.dart';
import 'package:video_player/video_player.dart';
import '../services/loopback_media.dart';
import '../services/vault_service.dart';
import '../services/video_frames.dart';
import '../theme.dart';

/// Одновременно играет только одно: пока идёт одно вложение, другое останавливается.
class PlaybackHub {
  static final ValueNotifier<Object?> owner = ValueNotifier(null);
  static void claim(Object o) => owner.value = o;
  static void release(Object o) {
    if (owner.value == o) owner.value = null;
  }
}

String _mmss(Duration d) => '${d.inMinutes}:${(d.inSeconds % 60).toString().padLeft(2, '0')}';

/// Голосовое/аудио прямо в сообщении, как в Telegram: круглая кнопка, полоса, время. Тап — и пошло.
class InlineAudio extends StatefulWidget {
  final String path;
  final int size;
  final String? caption;
  /// Для тестов: источник байтов вместо хранилища.
  final Future<Uint8List> Function(String path)? loadBytes;
  /// Для тестов: готовый плеер.
  final AudioPlayer Function()? playerFactory;
  const InlineAudio({super.key, required this.path, required this.size, this.caption, this.loadBytes, this.playerFactory});

  /// Дальше этого размера звук внутри сообщения не загружается целиком: открывается экран просмотра.
  static const maxInline = 40 * 1024 * 1024;

  @override
  State<InlineAudio> createState() => _InlineAudioState();
}

class _InlineAudioState extends State<InlineAudio> {
  AudioPlayer? _p;
  Duration _pos = Duration.zero, _dur = Duration.zero;
  bool _playing = false, _loaded = false, _busy = false;
  final _subs = <StreamSubscription<dynamic>>[];

  @override
  void initState() {
    super.initState();
    PlaybackHub.owner.addListener(_onHub);
  }

  void _onHub() {
    if (PlaybackHub.owner.value != this && _playing) {
      _p?.pause();
    }
  }

  AudioPlayer _player() {
    final p = _p;
    if (p != null) return p;
    final n = _p = widget.playerFactory?.call() ?? AudioPlayer();
    _subs.add(n.onPositionChanged.listen((d) { if (mounted) setState(() => _pos = d); }));
    _subs.add(n.onDurationChanged.listen((d) { if (mounted) setState(() => _dur = d); }));
    _subs.add(n.onPlayerStateChanged.listen((s) {
      if (!mounted) return;
      setState(() => _playing = s == PlayerState.playing);
      if (s == PlayerState.completed) {
        setState(() { _pos = Duration.zero; _playing = false; });
        PlaybackHub.release(this);
      }
    }));
    return n;
  }

  Future<void> _toggle() async {
    if (_busy) return;
    final p = _player();
    if (_playing) {
      await p.pause();
      return;
    }
    PlaybackHub.claim(this);
    if (_loaded) {
      if (_dur > Duration.zero && _pos >= _dur) await p.seek(Duration.zero);
      await p.resume();
      return;
    }
    setState(() => _busy = true);
    try {
      final bytes = await (widget.loadBytes != null
          ? widget.loadBytes!(widget.path)
          : (await VaultService.instance.files()).readAll(File(widget.path)));
      await p.play(BytesSource(bytes));
      _loaded = true;
    } catch (_) {
      if (mounted) ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Не удалось воспроизвести: файл повреждён')));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  void dispose() {
    PlaybackHub.owner.removeListener(_onHub);
    PlaybackHub.release(this);
    for (final s in _subs) { s.cancel(); }
    _p?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final total = _dur.inMilliseconds;
    final value = total == 0 ? 0.0 : _pos.inMilliseconds.clamp(0, total).toDouble();
    return Row(mainAxisSize: MainAxisSize.min, children: [
      GestureDetector(
        key: const ValueKey('audio-play'),
        onTap: _toggle,
        child: Container(
          width: 44,
          height: 44,
          decoration: const BoxDecoration(shape: BoxShape.circle, color: AppTheme.accent),
          child: _busy
              ? const Padding(padding: EdgeInsets.all(12), child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white))
              : Icon(_playing ? Icons.pause : Icons.play_arrow, color: Colors.white, size: 28),
        ),
      ),
      const SizedBox(width: 8),
      SizedBox(
        width: 170,
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
          if (widget.caption != null)
            Text(widget.caption!, maxLines: 1, overflow: TextOverflow.ellipsis, style: const TextStyle(color: AppTheme.text, fontSize: 13)),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 3,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 5),
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 12),
              activeTrackColor: AppTheme.accent,
              thumbColor: AppTheme.accent,
              inactiveTrackColor: Colors.white24,
            ),
            child: SizedBox(
              height: 24,
              child: Slider(
                key: const ValueKey('audio-seek'),
                value: value,
                max: total == 0 ? 1 : total.toDouble(),
                onChanged: total == 0 || !_loaded ? null : (v) => _p?.seek(Duration(milliseconds: v.round())),
              ),
            ),
          ),
          Text(total == 0 ? '0:00' : '${_mmss(_pos)} / ${_mmss(_dur)}',
              key: const ValueKey('audio-time'), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 11)),
        ]),
      ),
    ]);
  }
}

/// Видео в сообщении: сначала кадр с кнопкой «играть»; тап — воспроизведение здесь же, тап по видео — пауза.
class InlineVideo extends StatefulWidget {
  final String path;
  final int size;
  final String mime;
  final VoidCallback? onFullscreen;
  const InlineVideo({super.key, required this.path, required this.size, required this.mime, this.onFullscreen});

  @override
  State<InlineVideo> createState() => _InlineVideoState();
}

class _InlineVideoState extends State<InlineVideo> {
  Uint8List? _frame;
  bool _frameDone = false;
  VideoPlayerController? _c;
  LoopbackMedia? _media;
  bool _starting = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _frame = VideoFrames.cached(widget.path);
    _frameDone = _frame != null;
    if (!_frameDone) {
      VideoFrames.of(widget.path, widget.size, widget.mime).then((f) {
        if (mounted) setState(() { _frame = f; _frameDone = true; });
      });
    }
    PlaybackHub.owner.addListener(_onHub);
  }

  void _onHub() {
    if (PlaybackHub.owner.value != this && (_c?.value.isPlaying ?? false)) _c?.pause();
  }

  Future<void> _start() async {
    if (_starting) return;
    setState(() { _starting = true; _error = null; });
    PlaybackHub.claim(this);
    try {
      final vault = await VaultService.instance.files();
      final file = File(widget.path);
      final m = LoopbackMedia.reader(widget.size, widget.mime, (s, n) => vault.readRange(file, s, n));
      _media = m;
      final url = await m.start();
      if (!mounted) { await m.stop(); return; }   // виджет закрыт, пока поднимался сервер: плеер не создаём
      final c = VideoPlayerController.networkUrl(url);
      _c = c;
      await c.initialize();
      if (!mounted) return;                         // dispose виджета уже освободил _c и _media
      c.addListener(() { if (mounted) setState(() {}); });
      if (mounted) setState(() {});
      await c.play();
    } catch (_) {
      if (mounted) setState(() => _error = 'Не удалось воспроизвести');
    } finally {
      if (mounted) setState(() => _starting = false);
    }
  }

  @override
  void dispose() {
    PlaybackHub.owner.removeListener(_onHub);
    PlaybackHub.release(this);
    _c?.dispose();
    _media?.stop();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = _c;
    final ready = c != null && c.value.isInitialized;
    final aspect = ready ? c.value.aspectRatio : 16 / 9;
    return ClipRRect(
      borderRadius: BorderRadius.circular(10),
      child: SizedBox(
        width: 240,
        child: AspectRatio(
          aspectRatio: aspect <= 0 ? 16 / 9 : aspect,
          child: GestureDetector(
            key: const ValueKey('video-inline'),
            behavior: HitTestBehavior.opaque,
            onTap: () {
              if (!ready) {
                _start();
              } else if (c.value.isPlaying) {
                c.pause();
              } else {
                PlaybackHub.claim(this);
                c.play();
              }
            },
            child: Stack(fit: StackFit.expand, children: [
              Container(color: Colors.black),
              if (ready)
                VideoPlayer(c)
              else if (_frame != null)
                Image.memory(_frame!, fit: BoxFit.cover, gaplessPlayback: true)
              else
                Center(child: Icon(_frameDone ? Icons.movie_outlined : Icons.hourglass_empty, color: Colors.white38, size: 40)),
              if (!ready || !c.value.isPlaying)
                Center(
                  child: Container(
                    width: 56,
                    height: 56,
                    decoration: const BoxDecoration(color: Color(0x88000000), shape: BoxShape.circle),
                    child: _starting
                        ? const Padding(padding: EdgeInsets.all(14), child: CircularProgressIndicator(strokeWidth: 2.5, color: Colors.white))
                        : const Icon(Icons.play_arrow, color: Colors.white, size: 38, key: ValueKey('video-inline-play')),
                  ),
                ),
              if (_error != null)
                Positioned(left: 8, right: 8, bottom: 8, child: Text(_error!, style: const TextStyle(color: Colors.redAccent, fontSize: 12))),
              if (ready)
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  child: Row(children: [
                    Expanded(child: VideoProgressIndicator(c, allowScrubbing: true, padding: const EdgeInsets.fromLTRB(0, 14, 0, 0),
                        colors: VideoProgressColors(playedColor: AppTheme.accent, bufferedColor: Colors.white30, backgroundColor: Colors.white12))),
                    if (widget.onFullscreen != null)
                      GestureDetector(
                        onTap: widget.onFullscreen,
                        child: const Padding(padding: EdgeInsets.fromLTRB(4, 0, 6, 4), child: Icon(Icons.fullscreen, color: Colors.white, size: 22)),
                      ),
                  ]),
                ),
            ]),
          ),
        ),
      ),
    );
  }
}
