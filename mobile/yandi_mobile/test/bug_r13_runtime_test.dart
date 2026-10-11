import 'dart:io';
import 'package:flutter_test/flutter_test.dart';

void main() {
  for (final runner in ['db', 'services', 'lifecycle']) {
    test('bug_r13_${runner}_regressions_from_current_source', () async {
      final result = await Process.run('python3', ['test/run_r13_$runner.py']);
      expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
      expect(result.stdout, contains('0 FAIL'));
    }, timeout: const Timeout(Duration(seconds: 90)));
  }
}
