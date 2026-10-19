"""Исполняются текущие методы сервисов с хранилищем в памяти и управляемым IO.
Это не проверка Flutter UI, Android Keystore, SQLCipher и физического wipe.
"""
import json,re,subprocess,tempfile
from pathlib import Path
APP=Path(__file__).resolve().parents[1]
def src(name):return (APP/'lib'/name).read_text()
def strip(s):return re.sub(r'^import .*;\n','',s,flags=re.M)
def method(s,sig):
 start=s.index(sig);return s[start:s.index('\n  }',start)+4]
storage=src('services/storage_service.dart')
code="import 'dart:collection';\nimport 'dart:async';\nimport 'dart:convert';\nimport 'dart:io';\nimport 'dart:math';\nimport 'dart:typed_data';\nimport 'package:cryptography/cryptography.dart';\nimport 'package:path/path.dart' as p;\nimport 'package:crypto/crypto.dart' show sha256;\n"
for f in ['crypto/file_vault.dart','crypto/replay_ledger.dart','models/avatar_msg.dart','models/file_offer.dart','l10n_core.dart']:
 code+=f"import '{(APP/'lib'/f).as_uri()}';\n"
code+='''
class ChangeNotifier {void notifyListeners(){} }
class FileTransferService {static String safeName(String s)=>s.replaceAll('/','_');static String guessMime(String s)=>'text/plain';}
class RawSecure {
 final Map<String,String> kv={}; String? failWrite;
 Future<String?> read({required String key})async=>kv[key];
 Future<Map<String,String>> readAll()async=>Map.of(kv);
 Future<void> write({required String key,required String value})async{if(key==failWrite){failWrite=null;throw StateError("write failed");}kv[key]=value;}
 Future<void> delete({required String key})async{kv.remove(key);}
}
class StorageService {
 static final _secure=RawSecure(); static int cleanupFailures=0;
'''
code+=' static final db=FakeDb();\n'
code+=storage[storage.index('  static const _wrapPrefix'):storage.index('  // ── Ноды')]
code+=method(storage,'  static Future<String> ledgerSeal') if False else '  static Future<String> ledgerSeal(String json) => _wrap(\'ledger:replay\', json);\n'
# Выражения и соседние методы копируются из актуального исходника вместе.
start=storage.index('  static const _kLedgerSealed');end=storage.index('  /// «Удалить всё»',start)
code=code[:code.rfind('  static Future<String> ledgerSeal')]+storage[start:end]+'\n}\n'
code+=strip(src('services/vault_files.dart'))+strip(src('services/avatar_service.dart'))
code+='''
class VaultService {
 static final instance=VaultService(); FileVault? current;
 Future<FileVault> files()async {if(current==null)throw StateError('locked');return current!;}
}
'''+strip(src('services/vault_bytes_cache.dart'))
app=src('services/app_state.dart')
code+=strip(src('services/prep_tracker.dart'))
code+='''
class FakeDb {final rows=<Map<String,Object?>>[];Future<List<Map<String,Object?>>> query(String t,{List<String>? columns,String? where,List<String>? whereArgs})async=>rows;}
class pp {static late Directory cache;static Future<Directory> getTemporaryDirectory()async=>cache;}
class AppState extends ChangeNotifier {
 late VaultFiles vf; AvatarService? avatars; String? protectionWarning;static Future<bool> Function(String)? hook;
 final CopyOwners _copies = CopyOwners();
 static Future<bool> wipeFile(String s)async=>await hook?.call(s)??false;
 Future<VaultFiles> vaultFiles()async=>vf;
'''
for sig in ['  Future<void> _filesHousekeeping()', '  Future<String?> _legacyIdFor(', '  Future<void> _sweepPickerCache()', '  Future<void> dropPickerCopy(']:code+=method(app,sig)+'\n'
code+='}\n'
code+=(APP/'test/r13_services_cases.dart.inc').read_text()
with tempfile.TemporaryDirectory(prefix='r14_services-') as td:
 f=Path(td)/'probe.dart';f.write_text(code)
 raise SystemExit(subprocess.run(['/home/iam/.local/flutter-sdk/bin/cache/dart-sdk/bin/dart',f'--packages={APP / ".dart_tool/package_config.json"}',str(f)],cwd=APP).returncode)
