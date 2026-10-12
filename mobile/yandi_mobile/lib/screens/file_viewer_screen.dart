import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:audioplayers/audioplayers.dart';
import 'package:video_player/video_player.dart';
import '../services/loopback_media.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';
import '../services/vault_bytes_cache.dart';
import '../services/vault_files.dart';
import '../services/vault_service.dart';
import '../theme.dart';

/// Просмотр файла внутри приложения, без передачи другим программам: картинки, текст, звук. Остальное — только сведения о файле.
/// Файл расшифровывается в памяти; открытой копии на диске нет.
class FileViewerScreen extends StatefulWidget {
  final VaultFile file;
  final VoidCallback? onShare;
  final Future<void> Function()? onDelete;
  const FileViewerScreen({super.key, required this.file, this.onShare, this.onDelete});

  @override
  State<FileViewerScreen> createState() => _FileViewerScreenState();
}

class _FileViewerScreenState extends State<FileViewerScreen> {
  AudioPlayer? _player;
  VideoPlayerController? _video;
  LoopbackMedia? _media;
  String? _videoError;
  Duration _pos = Duration.zero, _dur = Duration.zero;
  bool _playing = false;
  final _subs = <StreamSubscription<dynamic>>[];

  VaultFile get f => widget.file;

  @override
  void initState() {
    super.initState();
    if (f.isVideo) {
      _openVideo();
    }
    if (f.isAudio) {
      final p = AudioPlayer();
      _player = p;
      _subs.add(p.onPositionChanged.listen((d) { if (mounted) setState(() => _pos = d); }));
      _subs.add(p.onDurationChanged.listen((d) { if (mounted) setState(() => _dur = d); }));
      _subs.add(p.onPlayerStateChanged.listen((s) { if (mounted) setState(() => _playing = s == PlayerState.playing); }));
    }
  }

  /// Видео внутри защищённого пространства: файл расшифровывается в памяти и отдаётся плееру по 127.0.0.1 под неугадываемым путём
  /// (с Range для перемотки). Открытой копии на диске нет; после закрытия экрана память затирается.
  Future<void> _openVideo() async {
    try {
      final vault = await VaultService.instance.files();
      final file = File(f.path);
      // Куски расшифровываются по запросам плеера: размер видео не ограничен (фильм в гигабайт), в памяти — один кусок.
      final m = LoopbackMedia.reader(f.size, f.mime, (start, len) => vault.readRange(file, start, len));
      _media = m;
      final url = await m.start();
      final c = VideoPlayerController.networkUrl(url);
      _video = c;
      await c.initialize();
      c.addListener(() { if (mounted) setState(() {}); });
      if (mounted) setState(() {});
      await c.play();
    } catch (_) {
      if (mounted) setState(() => _videoError = 'Не удалось воспроизвести: файл повреждён или формат не поддерживается');
    }
  }

  @override
  void dispose() {
    _video?.dispose();
    _media?.stop();
    for (final s in _subs) { s.cancel(); }
    _player?.dispose();
    super.dispose();
  }

  static String humanSize(int b) {
    if (b < 1024) return '$b Б';
    if (b < 1024 * 1024) return '${(b / 1024).toStringAsFixed(1)} КБ';
    return '${(b / 1024 / 1024).toStringAsFixed(1)} МБ';
  }

  static String _mmss(Duration d) => '${d.inMinutes.toString().padLeft(2, '0')}:${(d.inSeconds % 60).toString().padLeft(2, '0')}';

  /// Первые 256 КБ как текст (больше на экране не нужно и память не жалко).
  Future<String> _readText() async {
    try {
      const limit = 256 * 1024;
      final bytes = await (await VaultService.instance.files()).readPrefix(File(f.path), limit);
      final text = utf8.decode(bytes, allowMalformed: true);
      return f.size > limit ? '$text\n\n… показаны первые 256 КБ из ${humanSize(f.size)}' : text;
    } catch (_) {
      return 'Не удалось прочитать файл';
    }
  }

  Widget _body() {
    if (f.isImage) {
      return InteractiveViewer(
        maxScale: 6,
        child: Center(
          child: FutureBuilder<Uint8List?>(
            future: VaultBytesCache.load(f.path),
            builder: (_, snap) {
              final b = snap.data;
              if (b == null) return snap.connectionState == ConnectionState.done ? _info('Не удалось показать картинку: файл повреждён или слишком большой') : const CircularProgressIndicator(color: AppTheme.accent);
              return Image.memory(b, errorBuilder: (_, __, ___) => _info('Не удалось показать картинку: файл повреждён или формат не поддерживается'));
            },
          ),
        ),
      );
    }
    if (f.isText) {
      return FutureBuilder<String>(
        future: _readText(),
        builder: (c, snap) => SingleChildScrollView(
          padding: const EdgeInsets.all(16),
          child: SelectableText(snap.data ?? 'Чтение…', style: const TextStyle(color: AppTheme.text, fontSize: 14)),
        ),
      );
    }
    if (f.isAudio) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Icon(Icons.audiotrack_outlined, size: 72, color: AppTheme.textSecondary),
            const SizedBox(height: 16),
            Text(f.name, textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.text, fontSize: 16)),
            const SizedBox(height: 16),
            Slider(
              activeColor: AppTheme.accent,
              value: _dur.inMilliseconds == 0 ? 0 : _pos.inMilliseconds.clamp(0, _dur.inMilliseconds).toDouble(),
              max: _dur.inMilliseconds == 0 ? 1 : _dur.inMilliseconds.toDouble(),
              onChanged: (v) => _player?.seek(Duration(milliseconds: v.round())),
            ),
            Row(mainAxisAlignment: MainAxisAlignment.spaceBetween, children: [
              Text(_mmss(_pos), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
              Text(_mmss(_dur), style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            ]),
            const SizedBox(height: 8),
            IconButton(
              iconSize: 56,
              color: AppTheme.accent,
              icon: Icon(_playing ? Icons.pause_circle_filled : Icons.play_circle_filled),
              onPressed: () async {
                final p = _player;
                if (p == null) return;
                if (_playing) { await p.pause(); }
                else if (_pos > Duration.zero && _pos < _dur) { await p.resume(); }
                else {
                  try {
                    await p.play(BytesSource(await (await VaultService.instance.files()).readAll(File(f.path))));
                  } catch (_) {
                    if (mounted) ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Не удалось воспроизвести: файл повреждён')));
                  }
                }
              },
            ),
          ]),
        ),
      );
    }
    if (f.isVideo) {
      final c = _video;
      if (_videoError != null) return _info(_videoError!);
      if (c == null || !c.value.isInitialized) return const Center(child: CircularProgressIndicator());
      return Column(children: [
        Expanded(child: Center(child: AspectRatio(aspectRatio: c.value.aspectRatio, child: VideoPlayer(c)))),
        VideoProgressIndicator(c, allowScrubbing: true, colors: VideoProgressColors(playedColor: AppTheme.accent)),
        Row(mainAxisAlignment: MainAxisAlignment.center, children: [
          IconButton(
            iconSize: 48,
            color: AppTheme.accent,
            icon: Icon(c.value.isPlaying ? Icons.pause_circle_filled : Icons.play_circle_filled),
            onPressed: () => c.value.isPlaying ? c.pause() : c.play(),
          ),
        ]),
        const SizedBox(height: 12),
      ]);
    }
    return _info(f.isVideo
        ? 'Видео внутри приложения пока не воспроизводится. Файл остаётся в защищённом пространстве; его можно переслать контакту YANDI.'
        : 'Этот тип файла внутри приложения не открывается. Файл остаётся в защищённом пространстве; его можно переслать контакту YANDI.');
  }

  Widget _info(String text) => Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Icon(Icons.insert_drive_file_outlined, size: 72, color: AppTheme.textSecondary),
            const SizedBox(height: 16),
            Text(f.name, textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.text, fontSize: 16)),
            const SizedBox(height: 4),
            Text('${humanSize(f.size)} · ${DateFormat('dd.MM.yyyy HH:mm').format(f.modified)}', style: const TextStyle(color: AppTheme.textSecondary, fontSize: 12)),
            const SizedBox(height: 16),
            Text(text, textAlign: TextAlign.center, style: const TextStyle(color: AppTheme.textSecondary)),
          ]),
        ),
      );

  Future<void> _confirmDelete() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.surface,
        title: const Text('Удалить файл?', style: TextStyle(color: AppTheme.text)),
        content: Text('«${f.name}» будет удалён сразу и насовсем, без корзины.', style: const TextStyle(color: AppTheme.textSecondary)),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Отмена')),
          TextButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Удалить', style: TextStyle(color: Colors.redAccent))),
        ],
      ),
    );
    if (ok != true) return;
    await widget.onDelete?.call();
    if (mounted) Navigator.pop(context, true);
  }

  @override
  Widget build(BuildContext context) => Scaffold(
        appBar: AppBar(
          title: Text(f.name, overflow: TextOverflow.ellipsis),
          actions: [
            if (widget.onShare != null) IconButton(tooltip: 'Переслать контакту', icon: const Icon(Icons.share_outlined), onPressed: widget.onShare),
            if (widget.onDelete != null) IconButton(tooltip: 'Удалить', icon: const Icon(Icons.delete_outline, color: Colors.redAccent), onPressed: _confirmDelete),
          ],
        ),
        body: _body(),
      );
}
