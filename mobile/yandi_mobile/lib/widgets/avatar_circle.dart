import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../services/app_state.dart';
import '../theme.dart';

/// Круглая аватарка: картинка контакта, если она есть (только у сохранённых контактов), иначе первая буква имени.
class AvatarCircle extends StatelessWidget {
  final String? peerId;
  final bool mine;
  final String label;
  final double radius;
  /// false — картинку не показывать (человек ещё не в контактах): только буква.
  final bool showImage;
  const AvatarCircle({super.key, required String this.peerId, required this.label, this.radius = 24, this.showImage = true}) : mine = false;
  /// Своя картинка (в настройках).
  const AvatarCircle.mine({super.key, required this.label, this.radius = 24}) : peerId = null, mine = true, showImage = true;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final bytes = !showImage ? null : (mine ? state.myAvatarBytes : state.avatarBytes(peerId!));
    final initial = label.replaceFirst(RegExp(r'^\s*📱\s*'), '').trim();
    return CircleAvatar(
      radius: radius,
      backgroundColor: AppTheme.surface,
      backgroundImage: bytes != null ? MemoryImage(bytes) : null,
      onBackgroundImageError: bytes != null ? (_, __) {} : null,
      child: bytes != null
          ? null
          : Text(initial.isNotEmpty ? initial[0].toUpperCase() : '?',
              style: TextStyle(color: AppTheme.accent, fontWeight: FontWeight.bold, fontSize: radius * 0.75)),
    );
  }
}
