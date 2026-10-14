import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';

/// Круглый логотип группы: картинка, если задана, иначе первая буква названия.
class GroupLogo extends StatefulWidget {
  final String gid;
  final String name;
  final String hash;
  final double radius;
  const GroupLogo({super.key, required this.gid, required this.name, required this.hash, this.radius = 20});

  @override
  State<GroupLogo> createState() => _GroupLogoState();
}

class _GroupLogoState extends State<GroupLogo> {
  @override
  void initState() {
    super.initState();
    _fetch();
  }

  @override
  void didUpdateWidget(GroupLogo old) {
    super.didUpdateWidget(old);
    if (old.hash != widget.hash) _fetch();
  }

  void _fetch() {
    if (widget.hash.isEmpty) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) context.read<AppState>().ensureGroupLogo(widget.gid, widget.hash);
    });
  }

  @override
  Widget build(BuildContext context) {
    final bytes = widget.hash.isEmpty ? null : context.watch<AppState>().groupLogo(widget.gid, widget.hash);
    final letter = widget.name.isEmpty ? '?' : widget.name.characters.first.toUpperCase();
    return CircleAvatar(
      radius: widget.radius,
      backgroundColor: AppTheme.surface,
      backgroundImage: bytes != null ? MemoryImage(bytes) : null,
      onBackgroundImageError: bytes != null ? (_, __) {} : null,
      child: bytes == null ? Text(letter, style: const TextStyle(color: AppTheme.accent)) : null,
    );
  }
}
