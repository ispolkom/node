import 'dart:io';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/file_vault.dart';
import 'package:yandi_mobile/screens/files_tab.dart';
import 'package:yandi_mobile/services/vault_files.dart';
import 'files_tab_test.dart' show FakeHost;

class _Picker extends FilePicker {
  _Picker(this.path);
  final String path;
  int clears = 0;
  @override
  Future<FilePickerResult?> pickFiles({String? dialogTitle, String? initialDirectory,
    FileType type = FileType.any, List<String>? allowedExtensions,
    Function(FilePickerStatus)? onFileLoading, bool allowCompression = true,
    int compressionQuality = 30, bool allowMultiple = false, bool withData = false,
    bool withReadStream = false, bool lockParentWindow = false, bool readSequential = false}) async =>
      FilePickerResult([PlatformFile(path: path, name: 'chosen.txt', size: 7)]);
  @override
  Future<bool?> clearTemporaryFiles() async { clears++; throw StateError('обычный unlink запрещён'); }
}

class _Vault extends VaultFiles {
  _Vault(Directory dir, this.fail) : super(dir, vault: () async => FileVault.withKey(List.filled(32, 7)));
  final bool fail;
  int imports = 0;
  @override
  Future<List<VaultFile>> list() async => [];
  @override
  Future<void> finishPendingWipes() async {}
  @override
  Future<VaultFile> importFile(String sourcePath, {String? name}) async {
    imports++;
    if (fail) throw StateError('нет места');
    return VaultFile(path: pathFor('a' * 32), id: 'a' * 32, name: name!, size: 7,
        modified: DateTime(2026), mime: 'text/plain');
  }
}

void main() {
  // В widget harness автогенерированная регистрация IO-плагина не запускается.
  FilePickerIO.registerWith();
  for (final fail in [false, true]) {
    testWidgets('bug_r13_picker_widget_wipes_on_${fail ? 'import_failure' : 'success'}', (tester) async {
      final dir = Directory.systemTemp.createTempSync('r14_picker_widget_');
      final original = FilePicker.platform;
      final file = File('${dir.path}/chosen.txt')..writeAsStringSync('private');
      final picker = _Picker(file.path);
      FilePicker.platform = picker;
      final vault = _Vault(dir, fail);
      final wiped = <String>[];
      final key = GlobalKey<FilesTabState>();
      try {
        await tester.pumpWidget(MaterialApp(home: Scaffold(body: FilesTab(key: key, vault: vault, host: FakeHost(),
          dropPickerCopy: (path) async {
            wiped.add(path);
            expect(File(path).existsSync(), isTrue, reason: 'plugin не должен удалить копию перед wipe');
            await File(path).delete();
          }))));
        await tester.pump();
        await tester.runAsync(() => key.currentState!.addFiles());
        await tester.pump();
        expect(vault.imports, 1);
        expect(wiped, [file.path]);
        expect(file.existsSync(), isFalse);
        expect(picker.clears, 0);
      } finally {
        await tester.pumpWidget(const SizedBox());
        FilePicker.platform = original;
        await tester.runAsync(() => dir.delete(recursive: true));
      }
    });
  }
}
