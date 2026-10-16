import 'theme.dart';
import 'preferences.dart';
import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'domain/file_repository.dart';
import 'domain/shell_repository.dart';
import 'platform_files.dart';
import 'transport/api.dart';

/// Only daemon bytes in memory; never fetch an avatar URL supplied by a profile.
class ProfilePanel extends StatefulWidget {
  final Api api;
  final String? peer;
  final bool busy;
  final bool Function() isActive;
  final Future<void> Function(Future<void> Function()) mutate;
  /// Изменить имя и аватар собеседника в моей книжке (только у меня).
  final VoidCallback? onEdit;
  const ProfilePanel({
    super.key,
    this.onEdit,
    required this.api,
    this.peer,
    required this.busy,
    required this.isActive,
    required this.mutate,
  });
  @override
  State<ProfilePanel> createState() => _ProfilePanelState();
}

class _ProfilePanelState extends State<ProfilePanel> {
  Map<String, dynamic>? profile;
  Uint8List? data;
  MemoryImage? image;
  String? error;
  bool loading = false, choosing = false;
  bool get active => mounted && widget.isActive();
  bool get own => widget.peer == null;
  @override
  void initState() {
    super.initState();
    load();
  }

  void wipe() {
    image?.evict();
    image = null;
    data?.fillRange(0, data!.length, 0);
    data = null;
  }

  @override
  void dispose() {
    wipe();
    profile = null;
    super.dispose();
  }

  Future<void> load() async {
    if (!active || loading) return;
    setState(() {
      loading = true;
      error = null;
    });
    try {
      final result = await widget.api.request(
        own
            ? '/api/profile/me'
            : '/api/profile/${ShellRepository(widget.api).segment(widget.peer!)}',
      );
      if (!active) return;
      if (result['profile'] is! Map<String, dynamic>)
        throw const ApiFailure('protocol');
      setState(() {
        profile = result['profile'] as Map<String, dynamic>;
      });
      // картинка собеседника показывается сразу, без отдельной кнопки «Просмотреть аватар»
      if (!own || (profile?['avatar'] is String && (profile!['avatar'] as String).isNotEmpty)) {
        unawaited(preview());
      }
    } catch (e) {
      if (active)
        setState(() {
          error = e is ApiFailure ? e.message : 'Профиль недоступен.';
        });
    } finally {
      if (active)
        setState(() {
          loading = false;
        });
    }
  }

  Future<void> preview() async {
    final id = widget.peer ?? profile?['short_id'];
    if (id is! String ||
        !RegExp(r'^[a-fA-F0-9]{16}([a-fA-F0-9]{48})?$').hasMatch(id))
      return;
    try {
      final bytes = await FileRepository(
        widget.api,
      ).binary.bytes('/api/avatar/$id', limit: 4 * 1024 * 1024);
      if (!active) {
        bytes.fillRange(0, bytes.length, 0);
        return;
      }
      if (!FileRepository.raster(bytes)) {
        bytes.fillRange(0, bytes.length, 0);
        throw const ApiFailure('avatar_invalid');
      }
      setState(() {
        wipe();
        data = bytes;
        image = MemoryImage(bytes);
      });
    } catch (_) {
      // у собеседника может не быть картинки — остаётся заглушка
    }
  }

  Future<void> upload() async {
    if (choosing || widget.busy) return;
    choosing = true;
    try {
      final path = await PlatformFiles.choose();
      if (!active || path == null) return;
      await widget.mutate(() async {
        final file = File(path), handle = await File(path).open();
        Uint8List? bytes;
        try {
          final size = await handle.length();
          // Existing multipart framework limit includes headers; match web UI 2 MiB.
          if (size == 0 || size >= 2 * 1024 * 1024)
            throw const ApiFailure('avatar_limit');
          bytes = await handle.read(size);
          if (!active) return;
          if (!FileRepository.raster(bytes) ||
              FileRepository.mime(path) == 'image/webp')
            throw const ApiFailure('avatar_invalid');
          await FileRepository(widget.api).binary.multipart(
            '/api/profile/avatar',
            {},
            'avatar',
            file.uri.pathSegments.last,
            bytes,
          );
          if (!active) return;
          setState(() {
            wipe();
            data = Uint8List.fromList(bytes!);
            image = MemoryImage(data!);
          });
          await load();
        } finally {
          bytes?.fillRange(0, bytes.length, 0);
          await handle.close();
        }
      });
    } catch (e) {
      if (active)
        setState(() {
          error = e is ApiFailure ? e.message : 'Не удалось выбрать аватар.';
        });
    } finally {
      choosing = false;
    }
  }

  Future<void> remove() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const UiText('Удалить аватар профиля?'),
        content: const UiText(
          'Аватар перестанет отображаться в профиле. API демона сохраняет прежний файл на узле.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const UiText('Отмена'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const UiText('Удалить аватар'),
          ),
        ],
      ),
    );
    if (!active || confirmed != true) return;
    await widget.mutate(() async {
      await widget.api.request(
        '/api/profile/me',
        method: 'PUT',
        body: {'avatar': ''},
      );
      if (active) {
        setState(wipe);
        await load();
      }
    });
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (own) const UiText('Мой аватар', style: TextStyle(fontSize: 18)),
      if (own)
        const Padding(
          padding: EdgeInsets.symmetric(vertical: 6),
          child: UiText(
            'Ник и аватар увидят ваши контакты и участники ваших групп. Не указывайте ФИО и личные фото.',
            key: ValueKey('profile-warning'),
          ),
        ),
      if (loading) const LinearProgressIndicator(),
      if (error != null) UiText(error!),
      if (image == null)
        Container(
          width: 80,
          height: 80,
          margin: const EdgeInsets.symmetric(vertical: 12),
          decoration: const BoxDecoration(color: Icq.header, border: Icq.bevel),
          child: const Icon(
            Icons.person_outline,
            size: 48,
            color: Colors.white,
          ),
        ),
      if (image != null)
        Image(
          image: image!,
          width: 96,
          height: 96,
          fit: BoxFit.contain,
          errorBuilder: (_, _, _) => const UiText('Не удалось показать аватар'),
        ),
      if (profile != null) ...[
        if (!own) Text('${profile!['display_name'] ?? widget.peer}'),
        SelectableText('${profile!['short_id'] ?? widget.peer ?? ''}'),
        if (!own) UiText(profile!['online'] == true ? 'В сети' : 'Не в сети'),
      ],
      const SizedBox(height: 8),
      Wrap(
        spacing: 8,
        runSpacing: 8,
        children: [
          _btn(Icons.refresh, 'Обновить профиль', widget.busy || loading ? null : load),
          if (!own && widget.onEdit != null)
            _btn(Icons.edit_outlined, 'Изменить имя и аватар', widget.busy ? null : widget.onEdit),
          if (own) _btn(Icons.image_outlined, 'Загрузить аватар', widget.busy || choosing ? null : upload),
          if (own &&
              profile?['avatar'] is String &&
              (profile!['avatar'] as String).isNotEmpty)
            _btn(Icons.delete_outline, 'Удалить аватар профиля', widget.busy ? null : remove),
        ],
      ),
    ],
  );

  Widget _btn(IconData icon, String label, VoidCallback? onTap) => OutlinedButton.icon(
    onPressed: onTap,
    icon: Icon(icon, size: 18),
    label: UiText(label, style: const TextStyle(fontFamily: 'Tahoma', fontFamilyFallback: ['NotoColorEmoji'], decoration: TextDecoration.none)),
    style: OutlinedButton.styleFrom(
      side: const BorderSide(color: Icq.border),
      textStyle: const TextStyle(fontSize: 13, fontFamily: 'Tahoma', fontFamilyFallback: ['NotoColorEmoji'], decoration: TextDecoration.none),
    ),
  );
}
