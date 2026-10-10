import 'dart:math' as math;
import 'package:flutter/material.dart';

/// Фирменные значки меню: рисуются кодом (никаких картинок и сторонних пакетов), одна толщина линии, сетка 24×24.
enum YandiGlyph { contacts, groups, search, settings, blacklist, offline, folder }

class YandiIcon extends StatelessWidget {
  final YandiGlyph glyph;
  final double size;
  final Color? color;
  /// Подпись для озвучки и подсказки (вместо текста на вкладке).
  final String? label;
  const YandiIcon(this.glyph, {super.key, this.size = 24, this.color, this.label});

  @override
  Widget build(BuildContext context) {
    final c = color ?? IconTheme.of(context).color ?? Colors.white;
    final paint = SizedBox(width: size, height: size, child: CustomPaint(painter: YandiGlyphPainter(glyph, c)));
    return label == null ? ExcludeSemantics(child: paint) : Semantics(label: label, image: true, child: ExcludeSemantics(child: paint));
  }
}

class YandiGlyphPainter extends CustomPainter {
  final YandiGlyph glyph;
  final Color color;
  const YandiGlyphPainter(this.glyph, this.color);

  static const double _stroke = 1.8;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    canvas.scale(size.width / 24, size.height / 24);
    final line = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = _stroke
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    final fill = Paint()..color = color..style = PaintingStyle.fill;
    switch (glyph) {
      case YandiGlyph.contacts:
        _person(canvas, line, fill, cx: 12, top: 2.5, k: 1, filled: false);
      case YandiGlyph.groups:
        _person(canvas, line, fill, cx: 5.2, top: 6.2, k: 0.62, filled: false);
        _person(canvas, line, fill, cx: 18.8, top: 6.2, k: 0.62, filled: false);
        _person(canvas, line, fill, cx: 12, top: 2.5, k: 1, filled: true);
      case YandiGlyph.search:
        canvas.drawCircle(const Offset(10.5, 10.5), 6.3, line);
        canvas.drawLine(const Offset(15.2, 15.2), const Offset(20.5, 20.5), line);
      case YandiGlyph.settings:
        canvas.drawPath(_gear(), line);
        canvas.drawCircle(const Offset(12, 12), 3.1, line);
      case YandiGlyph.blacklist:
        // лист с загнутым внутрь уголком под 45° и одной чертой поперёк
        final sheet = Path()
          ..moveTo(6, 3) ..lineTo(14, 3) ..lineTo(19, 8) ..lineTo(19, 21) ..lineTo(6, 21) ..close();
        canvas.drawPath(sheet, line);
        canvas.drawPath(Path()..moveTo(14, 3)..lineTo(14, 8)..lineTo(19, 8), line);
        canvas.drawLine(const Offset(3.5, 3.5), const Offset(20.5, 20.5), line);
      case YandiGlyph.offline:
        // схематичный телефон с косой чертой
        canvas.drawRRect(RRect.fromRectAndRadius(const Rect.fromLTRB(6.5, 2.5, 17.5, 21.5), const Radius.circular(2.4)), line);
        canvas.drawLine(const Offset(10.3, 5.4), const Offset(13.7, 5.4), line);
        canvas.drawCircle(const Offset(12, 18.6), 0.5, fill);
        canvas.drawLine(const Offset(3.5, 3.5), const Offset(20.5, 20.5), line);
      case YandiGlyph.folder:
        final f = Path()
          ..moveTo(3, 6.5) ..quadraticBezierTo(3, 5, 4.5, 5) ..lineTo(8.6, 5) ..quadraticBezierTo(9.4, 5, 9.9, 5.6) ..lineTo(11.3, 7.5)
          ..lineTo(19.5, 7.5) ..quadraticBezierTo(21, 7.5, 21, 9) ..lineTo(21, 18) ..quadraticBezierTo(21, 19.5, 19.5, 19.5)
          ..lineTo(4.5, 19.5) ..quadraticBezierTo(3, 19.5, 3, 18) ..close();
        canvas.drawPath(f, line);
    }
    canvas.restore();
  }

  /// Человек по пояс: голова, шея, плечи с ровным срезом внизу. `k` — масштаб от верхней точки головы; `filled` — залить;
  void _person(Canvas canvas, Paint line, Paint fill, {required double cx, required double top, required double k, required bool filled}) {
    canvas.save();
    canvas.translate(cx, top);
    canvas.scale(k, k);
    // координаты внутри: голова r=4 с центром (0,4), шея, плечи до y=18.5
    final head = Rect.fromCircle(center: const Offset(0, 4), radius: 4);
    final body = Path()
      ..moveTo(-7.6, 18.5)
      ..cubicTo(-7.6, 13.2, -4.2, 11.2, -1.6, 10.8)
      ..lineTo(1.6, 10.8)
      ..cubicTo(4.2, 11.2, 7.6, 13.2, 7.6, 18.5)
      ..close();
    final lineScaled = Paint()
      ..color = line.color
      ..style = PaintingStyle.stroke
      ..strokeWidth = _stroke / k
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    if (filled) {
      canvas.drawOval(head, fill);
      canvas.drawPath(body, fill);
    } else {
      canvas.drawOval(head, lineScaled);
      canvas.drawPath(body, lineScaled);
    }
    canvas.restore();
  }

  Path _gear() {
    const teeth = 8;
    const rIn = 6.9, rOut = 9.6;
    final p = Path();
    for (var i = 0; i < teeth; i++) {
      final a = i * 2 * math.pi / teeth;
      const w = 2 * math.pi / teeth;
      final pts = <(double, double)>[
        (a - w * 0.34, rIn), (a - w * 0.20, rOut), (a + w * 0.20, rOut), (a + w * 0.34, rIn),
      ];
      for (final (ang, r) in pts) {
        final o = Offset(12 + r * math.cos(ang), 12 + r * math.sin(ang));
        i == 0 && ang == pts.first.$1 ? p.moveTo(o.dx, o.dy) : p.lineTo(o.dx, o.dy);
      }
    }
    return p..close();
  }

  @override
  bool shouldRepaint(YandiGlyphPainter old) => old.glyph != glyph || old.color != color;
}
