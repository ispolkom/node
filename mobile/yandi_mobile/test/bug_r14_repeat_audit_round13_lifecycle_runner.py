import re, subprocess, tempfile
from pathlib import Path
from bug_r14_repeat_audit_round12_packages import ensure_packages
APP=Path(__file__).resolve().parents[1]
packages=ensure_packages()
code="import 'dart:async';\nimport 'dart:io';\nimport 'dart:typed_data';\n"
for f in ['crypto/vault_keys.dart','crypto/file_vault.dart']:
 code+=f"import '{(APP/'lib'/f).as_uri()}';\n"
code+='''
class ChangeNotifier {void notifyListeners(){} }
class DeviceVaultBackend extends MemoryVaultBackend {}
class VaultBytesCache {static void clear(){} }
class StorageService {
 static bool opened=false, failClose=false, failWrap=false;static List<int>? secure;
 static final reached=<Completer<void>>[], release=<Completer<void>>[];static int calls=0;
 static void reset(){opened=false;failClose=false;failWrap=false;secure=null;reached.clear();release.clear();calls=0;}
 static void gate(){reached.add(Completer<void>());release.add(Completer<void>());}
 static void setSecureKey(List<int>? k){secure=k==null?null:List.of(k);}
 static Future<void> wrapExistingSecureValues()async{if(failWrap)throw StateError('wrap failed');}
 static Future<void> openEncrypted(List<int> k)async{final n=calls++;if(n<reached.length){reached[n].complete();await release[n].future;}opened=true;}
 static Future<void> closeDb()async{opened=false;if(failClose)throw StateError('native close failed');}
 static Future<String?> eraseAllLocalData()async{opened=false;secure=null;return null;}
}
'''
code+=re.sub(r'^import .*;\n','',(APP/'lib/services/vault_service.dart').read_text(),flags=re.M)
code+=(APP/'test/bug_r14_repeat_bug_r13_lifecycle.dart.inc').read_text()
with tempfile.TemporaryDirectory(prefix='r14_lifecycle-') as td:
 f=Path(td)/'probe.dart';f.write_text(code)
 raise SystemExit(subprocess.run(['/home/iam/.local/flutter-sdk/bin/cache/dart-sdk/bin/dart',f'--packages={packages}',str(f)],cwd=APP).returncode)
