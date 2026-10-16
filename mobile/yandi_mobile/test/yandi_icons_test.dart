import 'dart:io';
import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/widgets/yandi_icons.dart';

void main() {
  testWidgets('все значки рисуются, с подписью для озвучки', (t) async {
    await t.pumpWidget(MaterialApp(home: Scaffold(body: Wrap(children: [
      for (final g in YandiGlyph.values) YandiIcon(g, color: Colors.teal, label: g.name),
    ]))));
    for (final g in YandiGlyph.values) {
      expect(find.bySemanticsLabel(g.name), findsOneWidget, reason: g.name);
    }
    expect(tester(t), isNull);
  });

  testWidgets('превью для глаз: лист значков в PNG', (t) async {
    final key = GlobalKey();
    await t.binding.setSurfaceSize(const Size(560, 140));
    await t.pumpWidget(MaterialApp(debugShowCheckedModeBanner: false, home: Scaffold(
      backgroundColor: const Color(0xFF0D1117),
      body: RepaintBoundary(key: key, child: Container(color: const Color(0xFF0D1117), padding: const EdgeInsets.all(12), child: Column(children: [
        Row(children: [for (final g in YandiGlyph.values) Padding(padding: const EdgeInsets.all(6), child: YandiIcon(g, size: 56, color: const Color(0xFF00D4AA)))]),
        Row(children: [for (final g in YandiGlyph.values) Padding(padding: const EdgeInsets.all(6), child: YandiIcon(g, size: 24, color: const Color(0xFF7D8590)))]),
      ]))),
    )));
    await t.runAsync(() async {
      final b = key.currentContext!.findRenderObject() as RenderRepaintBoundary;
      final img = await b.toImage(pixelRatio: 3);
      final data = await img.toByteData(format: ui.ImageByteFormat.png);
      final bytes = data!.buffer.asUint8List();
      // лист значков действительно рисуется в PNG; файл никуда не сохраняется (нет жёстких путей и мусора на диске)
      expect(bytes.sublist(0, 4), [0x89, 0x50, 0x4E, 0x47]);
      expect(bytes.length, greaterThan(1000));
    });
  });
}

Object? tester(WidgetTester t) => t.takeException();
