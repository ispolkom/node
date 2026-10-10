import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/avatar_msg.dart';
import 'package:yandi_mobile/services/avatar_processor.dart';

int _crc32(List<int> data) {
  var c = 0xFFFFFFFF;
  for (final b in data) {
    c ^= b;
    for (var k = 0; k < 8; k++) { c = (c & 1) != 0 ? (0xEDB88320 ^ (c >> 1)) : (c >> 1); }
  }
  return c ^ 0xFFFFFFFF;
}

/// PNG с добавленным блоком tEXt после IHDR (как метаданные в настоящих фото).
Uint8List withTextChunk(Uint8List png, String keyword, String text) {
  final body = Uint8List.fromList([...keyword.codeUnits, 0, ...text.codeUnits]);
  final type = 'tEXt'.codeUnits;
  final len = ByteData(4)..setUint32(0, body.length);
  final crc = ByteData(4)..setUint32(0, _crc32([...type, ...body]));
  final chunk = [...len.buffer.asUint8List(), ...type, ...body, ...crc.buffer.asUint8List()];
  const ihdrEnd = 8 + 4 + 4 + 13 + 4;   // сигнатура + длина + тип + данные IHDR + CRC
  return Uint8List.fromList([...png.sublist(0, ihdrEnd), ...chunk, ...png.sublist(ihdrEnd)]);
}

bool contains(Uint8List hay, String needle) {
  final n = needle.codeUnits;
  for (var i = 0; i + n.length <= hay.length; i++) {
    var ok = true;
    for (var j = 0; j < n.length; j++) { if (hay[i + j] != n[j]) { ok = false; break; } }
    if (ok) return true;
  }
  return false;
}

({int w, int h}) pngSize(Uint8List p) {
  final b = ByteData.sublistView(p);
  return (w: b.getUint32(16), h: b.getUint32(20));
}

void main() {
  testWidgets('метаданные исходника не попадают в аватарку, размер 128×128, PNG', (t) async {
    await t.runAsync(() async {
      final src = await AvatarProcessor.fromEmoji('🙂');
      final marked = withTextChunk(src, 'GPS', 'SECRET-55.7558N-37.6173E-Infinix-X6517');
      expect(contains(marked, 'SECRET-55.7558N'), isTrue, reason: 'проверка самого теста: метка в исходнике есть');
      final out = await AvatarProcessor.fromImageBytes(marked);
      expect(AvatarMsg.validPng(out), isTrue);
      expect(contains(out, 'SECRET'), isFalse);
      expect(contains(out, 'tEXt'), isFalse);
      expect(contains(out, 'Infinix'), isFalse);
      final s = pngSize(out);
      expect((s.w, s.h), (128, 128));
    });
  });

  testWidgets('сложная картинка (шум) уменьшается, пока не влезет в лимит', (t) async {
    await t.runAsync(() async {
      final r = Random(1);
      final rgba = Uint8List(600 * 600 * 4);
      for (var i = 0; i < rgba.length; i++) { rgba[i] = r.nextInt(256); }
      final buf = await ui.ImmutableBuffer.fromUint8List(rgba);
      final desc = ui.ImageDescriptor.raw(buf, width: 600, height: 600, pixelFormat: ui.PixelFormat.rgba8888);
      final codec = await desc.instantiateCodec();
      final img = (await codec.getNextFrame()).image;
      final png = (await img.toByteData(format: ui.ImageByteFormat.png))!.buffer.asUint8List();
      final out = await AvatarProcessor.fromImageBytes(png);
      expect(out.length, lessThanOrEqualTo(AvatarMsg.maxPng));
      expect(AvatarMsg.validPng(out), isTrue);
      expect(pngSize(out).w, lessThanOrEqualTo(128));
    });
  });

  testWidgets('эмодзи: маленький валидный PNG', (t) async {
    await t.runAsync(() async {
      final p = await AvatarProcessor.fromEmoji('🐱');
      expect(AvatarMsg.validPng(p), isTrue);
      expect(p.length, lessThan(AvatarMsg.maxPng));
    });
  });

  testWidgets('не картинка → понятная ошибка, а не падение', (t) async {
    await t.runAsync(() async {
      await expectLater(AvatarProcessor.fromImageBytes(Uint8List.fromList(List.filled(100, 3))), throwsA(anything));
    });
  });
}
