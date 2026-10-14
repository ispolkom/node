import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import '../services/avatar_processor.dart';
import '../theme.dart';

/// Выбор логотипа группы: картинка из галереи собирается заново (квадрат, без метаданных) и отдаётся как base64.
/// [onPicked] получает base64 PNG либо пустую строку (убрать логотип).
class LogoPicker extends StatefulWidget {
  final Uint8List? current;
  final String groupName;
  final Future<void> Function(String logoB64, Uint8List? preview) onPicked;
  const LogoPicker({super.key, required this.current, required this.groupName, required this.onPicked});

  @override
  State<LogoPicker> createState() => _LogoPickerState();
}

class _LogoPickerState extends State<LogoPicker> {
  Uint8List? _preview;
  bool _busy = false;
  String? _error;

  Uint8List? get _shown => _preview ?? widget.current;

  Future<void> _pick() async {
    setState(() { _busy = true; _error = null; });
    try {
      final r = await FilePicker.platform.pickFiles(type: FileType.image, withData: false);
      final path = r?.files.single.path;
      if (path == null) return;
      final raw = await File(path).readAsBytes();
      if (raw.length > 12 * 1024 * 1024) { _error = 'Картинка слишком большая'; return; }
      final png = await AvatarProcessor.fromImageBytes(raw);
      await widget.onPicked(base64.encode(png), png);
      if (mounted) setState(() => _preview = png);
    } catch (e) {
      _error = 'Не удалось подготовить картинку';
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _remove() async {
    setState(() { _busy = true; _error = null; });
    try {
      await widget.onPicked('', null);
      if (mounted) setState(() => _preview = null);
    } catch (_) {
      _error = 'Не удалось убрать логотип';
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final letter = widget.groupName.isEmpty ? '?' : widget.groupName.characters.first.toUpperCase();
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Row(children: [
        CircleAvatar(
          radius: 28,
          backgroundColor: AppTheme.surface,
          backgroundImage: _shown != null ? MemoryImage(_shown!) : null,
          child: _shown == null ? Text(letter, style: const TextStyle(color: AppTheme.accent, fontSize: 22)) : null,
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Wrap(spacing: 8, children: [
            OutlinedButton.icon(
              onPressed: _busy ? null : _pick,
              icon: const Icon(Icons.image_outlined, size: 18),
              label: const Text('Логотип'),
            ),
            if (_shown != null)
              TextButton(onPressed: _busy ? null : _remove, child: const Text('Убрать', style: TextStyle(color: AppTheme.textSecondary))),
          ]),
        ),
      ]),
      if (_error != null) Padding(padding: const EdgeInsets.only(top: 6), child: Text(_error!, style: const TextStyle(color: AppTheme.incoming, fontSize: 12))),
    ]);
  }
}
