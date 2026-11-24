import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/app_version.dart';

void main() {
  test('kAppVersion совпадает с версией в pubspec.yaml (1.0.0+1 ↔ 1.0.0-b1)', () {
    final raw = RegExp(r'^version:\s*(\S+)', multiLine: true).firstMatch(File('pubspec.yaml').readAsStringSync())!.group(1)!;
    expect(kAppVersion, raw.replaceFirst('+', '-b'));
  });
}
