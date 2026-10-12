import 'preferences.dart';
import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'domain/file_repository.dart';
import 'platform_files.dart';
import 'transport/api.dart';

class AttachmentView extends StatefulWidget {
  final Api api;
  final Map<String, dynamic> attachment;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  final bool busy;
  final String? contentPath;
  const AttachmentView({
    super.key,
    required this.api,
    required this.attachment,
    required this.isActive,
    required this.mutate,
    required this.busy,
    this.contentPath,
  });
  @override
  State<AttachmentView> createState() => _AttachmentViewState();
}

class _AttachmentViewState extends State<AttachmentView> {
  late final repo = FileRepository(widget.api);
  Uint8List? preview;
  MemoryImage? image;
  bool ready = false, checking = false;
  String? error;
  Timer? watch;
  int attempts = 0;
  bool get active => mounted && widget.isActive();
  @override
  void initState() {
    super.initState();
    if (widget.contentPath != null) {
      ready = true;
    } else {
      check();
    }
  }

  @override
  void dispose() {
    watch?.cancel();
    image?.evict();
    preview?.fillRange(0, preview!.length, 0);
    preview = null;
    image = null;
    super.dispose();
  }

  Future<void> check() async {
    if (!active || checking) return;
    checking = true;
    watch?.cancel();
    try {
      final result = await widget.api.request(
        repo.path(widget.attachment, 'status'),
      );
      if (!active) return;
      if (result['exists'] is! bool) throw const ApiFailure('protocol');
      setState(() {
        ready = result['exists'] == true;
        error = null;
      });
      if (!ready && ++attempts < 5)
        watch = Timer(Duration(seconds: attempts * 2), check);
    } catch (e) {
      if (active)
        setState(() {
          error = e is ApiFailure ? e.message : 'Файл недоступен.';
        });
    } finally {
      checking = false;
    }
  }

  Future<void> showPreview() async {
    await widget.mutate(() async {
      final bytes = await repo.binary.bytes(
        widget.contentPath ?? repo.path(widget.attachment, 'content'),
      );
      if (!active) {
        bytes.fillRange(0, bytes.length, 0);
        return;
      }
      if (!FileRepository.raster(bytes)) {
        bytes.fillRange(0, bytes.length, 0);
        throw const ApiFailure('protocol');
      }
      setState(() {
        preview = bytes;
        image = MemoryImage(bytes);
      });
    });
  }

  Future<void> save(bool open) async {
    String? path;
    try {
      path = await PlatformFiles.choose(
        save: true,
        name: '${widget.attachment['filename']}',
      );
    } catch (_) {
      if (active)
        setState(() {
          error = 'Не удалось открыть диалог сохранения.';
        });
      return;
    }
    if (!active || path == null) return;
    await widget.mutate(() async {
      final size = widget.attachment['size'];
      if (size is! num || size < 0 || size > 1024 * 1024 * 1024)
        throw const ApiFailure('protocol');
      await repo.binary.download(
        widget.contentPath ?? repo.path(widget.attachment, 'content'),
        File(path!),
        limit: size.toInt(),
        isActive: () => active,
      );
      if (open && active) await PlatformFiles.open(path);
    });
  }

  @override
  Widget build(BuildContext context) {
    final type = FileRepository.mime('${widget.attachment['filename']}');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        UiText(
          '${widget.attachment['filename']} · ${widget.attachment['size']} байт',
        ),
        UiText(ready ? 'Файл доступен на узле' : 'Файл ещё не получен'),
        if (error != null) UiText(error!),
        if (image != null)
          Image(
            image: image!,
            width: 260,
            height: 200,
            fit: BoxFit.contain,
            errorBuilder: (_, _, _) =>
                const UiText('Не удалось показать картинку'),
          ),
        Wrap(
          children: [
            if (!ready)
              TextButton(
                onPressed: checking
                    ? null
                    : () {
                        attempts = 0;
                        check();
                      },
                child: const UiText('Проверить файл'),
              ),
            if (ready)
              TextButton(
                onPressed: widget.busy ? null : () => save(false),
                child: const UiText('Скачать'),
              ),
            if (ready && type.startsWith('image/') && image == null)
              TextButton(
                onPressed: widget.busy ? null : showPreview,
                child: const UiText('Предпросмотр'),
              ),
            if (ready &&
                (type.startsWith('image/') ||
                    type.startsWith('audio/') ||
                    type.startsWith('video/')))
              TextButton(
                onPressed: widget.busy ? null : () => save(true),
                child: const UiText('Сохранить и открыть'),
              ),
          ],
        ),
      ],
    );
  }
}
