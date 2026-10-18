import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/transport/api.dart';

void main() {
  test('restore failure keeps only bounded warnings and the files-written flag', () {
    final f = restoreFailureFrom(400, {
      'error': 'секретный текст ответа не должен попасть в исключение',
      'warnings': [
        'путь 0',
        42,
        null,
        'x' * 700,
        for (var i = 1; i < 30; i++) 'путь $i',
      ],
      'files_written': true,
      'token': 'abc',
    });
    expect(f.status, 400);
    expect(f.filesWritten, true);
    expect(f.warnings.length <= 20, true, reason: 'не больше 20 строк');
    expect(f.warnings.first, 'путь 0');
    expect(f.warnings.any((w) => w.length > 500), false, reason: 'длинная строка обрезана');
    expect(f.warnings.contains('42'), false, reason: 'не строки отброшены');
    expect(f.toString().contains('секретный'), false);
    final long = restoreFailureFrom(400, {'warnings': ['y' * 700]});
    expect(long.warnings.single.length, 500);
    final none = restoreFailureFrom(400, {'warnings': 'не список', 'files_written': 'true'});
    expect(none.warnings, isEmpty);
    expect(none.filesWritten, false, reason: 'только настоящий true');
  });
}
