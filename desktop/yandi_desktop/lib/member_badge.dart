import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'domain/file_repository.dart';
import 'message_actions.dart';
import 'transport/api.dart';

/// Кружок автора в ленте группы: картинка визитки, если участник её раскрыл, иначе первая буква имени.
/// Ответ (и «нет картинки») помнится минуту на пару группа+псевдоним, чтобы лента не опрашивала узел на каждый кадр.
class MemberBadge extends StatefulWidget {
  final Api api;
  final String gid;
  final String alias;
  final String name;
  final double size;
  const MemberBadge({
    super.key,
    required this.api,
    required this.gid,
    required this.alias,
    required this.name,
    this.size = 20,
  });

  static final Map<String, (DateTime, Uint8List?)> _cache = {};
  @visibleForTesting
  static void cacheForTest(String key, Uint8List png) => _cache[key] = (DateTime.now(), png);
  @visibleForTesting
  static int get cachedCount => _cache.length;

  static void forget() {
    for (final v in _cache.values) {
      final b = v.$2;
      if (b != null) b.fillRange(0, b.length, 0);
    }
    _cache.clear();
  }

  @override
  State<MemberBadge> createState() => _MemberBadgeState();
}

class _MemberBadgeState extends State<MemberBadge> {
  Uint8List? png;
  bool disposed = false;

  String get cacheKey => '${widget.gid}/${widget.alias}';

  @override
  void initState() {
    super.initState();
    unawaited(load());
  }

  @override
  void didUpdateWidget(MemberBadge old) {
    super.didUpdateWidget(old);
    if (old.gid != widget.gid || old.alias != widget.alias) {
      png = null;
      unawaited(load());
    }
  }

  @override
  void dispose() {
    disposed = true;
    super.dispose();
  }

  Future<void> load() async {
    final hit = MemberBadge._cache[cacheKey];
    if (hit != null &&
        DateTime.now().difference(hit.$1) < const Duration(minutes: 1)) {
      if (hit.$2 != null && mounted) setState(() => png = hit.$2);
      return;
    }
    Uint8List? got;
    try {
      final alias = Uri.encodeComponent(widget.alias);
      got = await FileRepository(widget.api).binary.bytes(
        '/api/groups/${widget.gid}/avatar/$alias',
        limit: 64 * 1024,
      );
    } catch (_) {
      got = null; // нет визитки, картинки или узел старой версии
    }
    MemberBadge._cache[cacheKey] = (DateTime.now(), got);
    if (!disposed && mounted && got != null) setState(() => png = got);
  }

  @override
  Widget build(BuildContext context) {
    final bytes = png;
    if (bytes == null || bytes.isEmpty) {
      return InitialBadge(widget.name, size: widget.size);
    }
    return ClipOval(
      child: Image.memory(
        bytes,
        width: widget.size,
        height: widget.size,
        fit: BoxFit.cover,
        errorBuilder: (_, _, _) => InitialBadge(widget.name, size: widget.size),
      ),
    );
  }
}
