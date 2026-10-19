import 'dart:io';
import 'package:flutter_test/flutter_test.dart';

void main() {
  // Контракт вызовов; нативное затирание на устройстве этот тест не имитирует.
  test('bug_r13_picker_import_and_avatar_must_wipe_before_plugin_unlink', () {
    for (final screen in ['settings_screen', 'chat_screen']) {
      final source = File('lib/screens/$screen.dart').readAsStringSync();
      expect(source, isNot(contains('clearTemporaryFiles(')), reason: screen);
      expect(source, contains('finally {'), reason: screen);
      expect(source, contains('dropPickerCopy('), reason: screen);
      final cleanup = source.substring(source.indexOf('finally {'));
      expect(cleanup, contains('dropPickerCopy('), reason: screen);
    }
  });

  test('вкладка «Файлы» ничего не берёт из памяти телефона: без выбора файлов и без кнопки «+»', () {
    final tab = File('lib/screens/files_tab.dart').readAsStringSync();
    expect(tab, isNot(contains('FilePicker')));
    expect(tab, isNot(contains('addFiles')));
    final home = File('lib/screens/home_screen.dart').readAsStringSync();
    expect(home, isNot(contains('addFiles')));
    expect(home, isNot(contains('Добавить файл в хранилище')));
  });
}
