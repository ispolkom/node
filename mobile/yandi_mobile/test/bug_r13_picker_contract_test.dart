import 'dart:io';
import 'package:flutter_test/flutter_test.dart';

void main() {
  // Контракт вызовов; нативное затирание на устройстве этот тест не имитирует.
  test('bug_r13_picker_import_and_avatar_must_wipe_before_plugin_unlink', () {
    for (final screen in ['files_tab', 'settings_screen', 'chat_screen']) {
      final source = File('lib/screens/$screen.dart').readAsStringSync();
      expect(source, isNot(contains('clearTemporaryFiles(')), reason: screen);
      expect(source, contains('finally {'), reason: screen);
      expect(source, screen == 'files_tab' ? contains('context.read<AppState>().dropPickerCopy') : contains('dropPickerCopy('), reason: screen);
      final cleanup = source.substring(source.indexOf('finally {'));
      expect(cleanup, screen == 'files_tab' ? contains('await cleanup(path)') : contains('dropPickerCopy('), reason: screen);
    }
  });
}
