import 'dart:typed_data';
import 'package:flutter/material.dart';
import '../services/vault_bytes_cache.dart';

/// Картинка из зашифрованного файла хранилища: расшифровывается в памяти (с небольшим кешем), на диске открытой копии нет.
class VaultImage extends StatelessWidget {
  final String path;
  final double? width, height;
  final int? cacheWidth;
  final BoxFit fit;
  final Widget Function(BuildContext)? fallback;
  const VaultImage({super.key, required this.path, this.width, this.height, this.cacheWidth, this.fit = BoxFit.cover, this.fallback});

  @override
  Widget build(BuildContext context) {
    final fb = fallback?.call(context) ?? const SizedBox.shrink();
    return FutureBuilder<Uint8List?>(
      key: ValueKey(path),
      future: VaultBytesCache.load(path),
      builder: (_, snap) {
        final b = snap.data;
        if (b == null) return SizedBox(width: width, height: height, child: snap.connectionState == ConnectionState.done ? fb : null);
        return Image.memory(b, width: width, height: height, cacheWidth: cacheWidth, fit: fit, gaplessPlayback: true, errorBuilder: (_, __, ___) => fb);
      },
    );
  }
}
