import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/transport/api.dart';

void main() {
  test('restore failure keeps only bounded warnings and the files-written flag', () {
    final f = restoreFailureFrom(400, {
      'error': 'секретный текст ответа не должен попасть в исключение',
      'warnings': [
        for (var i = 0; i < 30; i++) 'путь $i',
        42,
        null,
        'x' * 700,
      ],
      'files_written': true,
      'token': 'abc',
    });
    expect(f.status, 400);
    expect(f.filesWritten, true);
    expect(f.warnings.length, 20, reason: 'не больше 20 строк');
    expect(f.warnings.first, 'путь 0');
    expect(f.toString().contains('секретный'), false);
    final long = restoreFailureFrom(400, {'warnings': ['y' * 700]});
    expect(long.warnings.single.length, 500);
    final none = restoreFailureFrom(400, {'warnings': 'не список', 'files_written': 'true'});
    expect(none.warnings, isEmpty);
    expect(none.filesWritten, false, reason: 'только настоящий true');
  });
}
