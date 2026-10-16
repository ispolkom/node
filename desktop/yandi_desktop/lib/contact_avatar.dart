import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'domain/file_repository.dart';
import 'theme.dart';
import 'transport/api.dart';

/// Квадрат контакта в списке: его картинка, если она есть у узла, иначе первая буква имени.
/// Ответ (и «картинки нет») помнится полминуты на идентификатор, чтобы список не опрашивал узел на каждый кадр.
class ContactAvatar extends StatefulWidget {
  final Api api;
  final String avatarId;
  final String name;
  final double size;
  const ContactAvatar({
    super.key,
    required this.api,
    required this.avatarId,
    required this.name,
    this.size = 40,
  });

  static final Map<String, (DateTime, Uint8List?)> _cache = {};

  /// Картинку человека сменили (локально или он сам): забыть запомненный ответ, чтобы список запросил её заново.
  static void invalidate(String id) {
    _cache.remove(id);
    changes.value++;   // уже показанные картинки перечитываются
  }

  /// Счётчик смен картинок: показанные виджеты слушают его и перечитывают свою.
  static final ValueNotifier<int> changes = ValueNotifier<int>(0);

  @override
  State<ContactAvatar> createState() => _ContactAvatarState();
}

class _ContactAvatarState extends State<ContactAvatar> {
  Uint8List? png;

  void onChanged() {
    if (mounted) unawaited(load());
  }

  @override
  void initState() {
    super.initState();
    ContactAvatar.changes.addListener(onChanged);
    unawaited(load());
  }

  @override
  void dispose() {
    ContactAvatar.changes.removeListener(onChanged);
    super.dispose();
  }

  @override
  void didUpdateWidget(ContactAvatar old) {
    super.didUpdateWidget(old);
    if (old.avatarId != widget.avatarId) {
      png = null;
      unawaited(load());
    }
  }

  Future<void> load() async {
    final id = widget.avatarId;
    if (id.isEmpty) return;
    final hit = ContactAvatar._cache[id];
    if (hit != null && DateTime.now().difference(hit.$1) < const Duration(seconds: 30)) {
      if (hit.$2 != null && mounted) setState(() => png = hit.$2);
      return;
    }
    Uint8List? got;
    try {
      got = await FileRepository(widget.api).binary.bytes('/api/avatar/$id', limit: 256 * 1024);
    } catch (_) {
      got = null; // картинки нет
    }
    ContactAvatar._cache[id] = (DateTime.now(), got);
    if (mounted && got != null && id == widget.avatarId) setState(() => png = got);
  }

  @override
  Widget build(BuildContext context) {
    final name = widget.name.trim();
    final bytes = png;
    return Container(
      width: widget.size,
      height: widget.size,
      alignment: Alignment.center,
      decoration: const BoxDecoration(color: Icq.header, border: Icq.bevel),
      child: bytes != null && bytes.isNotEmpty
          ? Image.memory(
              bytes,
              width: widget.size,
              height: widget.size,
              fit: BoxFit.cover,
              errorBuilder: (_, _, _) => _letter(name),
            )
          : _letter(name),
    );
  }

  Widget _letter(String name) => Text(
    name.isEmpty ? '?' : name.characters.first.toUpperCase(),
    style: const TextStyle(color: Colors.white, fontSize: 18, fontWeight: FontWeight.bold),
  );
}
