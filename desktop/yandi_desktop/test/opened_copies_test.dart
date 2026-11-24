import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/voice.dart';

void main() {
  test('открытые копии вложений затираются и удаляются при запирании', () async {
    final dir = await Directory.systemTemp.createTemp('yandi_open_test');
    final f = File('${dir.path}/кот.png')..writeAsBytesSync(List.filled(64, 7));
    openedCopies.add(dir);
    await wipeOpenedCopies();
    expect(f.existsSync(), isFalse);
    expect(dir.existsSync(), isFalse);
    expect(openedCopies, isEmpty);
    await wipeOpenedCopies();   // повторный вызов безопасен
  });
}
