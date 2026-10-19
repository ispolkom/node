import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;
import 'package:flutter/painting.dart';
import '../models/avatar_msg.dart';
import '../l10n_core.dart';

/// Готовит аватарку к отправке. Картинка всегда СОБИРАЕТСЯ ЗАНОВО из пикселей (центральный квадрат, не больше 128×128, PNG), исходные
/// байты не копируются — поэтому в неё не попадают EXIF (GPS, время, модель телефона), XMP, встроенные эскизы и прочие метки.
class AvatarProcessor {
  static const sizes = [128, 96, 64];

  static Future<Uint8List> fromImageBytes(Uint8List source) async {
    // сначала уменьшаем при чтении (память), потом режем квадрат нужного размера
    final codec = await ui.instantiateImageCodec(source, targetWidth: 512);
    final frame = await codec.getNextFrame();
    final img = frame.image;
    try {
      for (final size in sizes) {
        final png = await _render(size, (c, rect) {
          final w = img.width.toDouble(), h = img.height.toDouble(), side = min(w, h);
          c.drawImageRect(img, ui.Rect.fromLTWH((w - side) / 2, (h - side) / 2, side, side), rect, ui.Paint()..filterQuality = ui.FilterQuality.medium);
        });
        if (png.length <= AvatarMsg.maxPng) return png;
      }
      throw StateError(tr('Картинка слишком сложная для аватарки'));
    } finally {
      img.dispose();
      codec.dispose();
    }
  }

  static Future<Uint8List> fromEmoji(String emoji, {ui.Color background = const ui.Color(0xFF1F2933)}) async {
    final size = sizes.first;
    return _render(size, (c, rect) {
      c.drawRect(rect, ui.Paint()..color = background);
      final tp = TextPainter(
        text: TextSpan(text: emoji, style: TextStyle(fontSize: size * 0.66)),
        textDirection: ui.TextDirection.ltr,
      )..layout();
      tp.paint(c, ui.Offset((size - tp.width) / 2, (size - tp.height) / 2));
    });
  }

  static Future<Uint8List> _render(int size, void Function(ui.Canvas, ui.Rect) draw) async {
    final rec = ui.PictureRecorder();
    final canvas = ui.Canvas(rec);
    draw(canvas, ui.Rect.fromLTWH(0, 0, size.toDouble(), size.toDouble()));
    final pic = rec.endRecording();
    final out = await pic.toImage(size, size);
    try {
      final data = await out.toByteData(format: ui.ImageByteFormat.png);
      if (data == null) throw StateError(tr('Не удалось закодировать картинку'));
      return data.buffer.asUint8List();
    } finally {
      out.dispose();
      pic.dispose();
    }
  }
}
