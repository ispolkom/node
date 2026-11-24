"""Регрессии настоящего VaultService с управляемыми задержками, без Android."""
import re
import subprocess
import tempfile
from pathlib import Path

APP = Path(__file__).resolve().parents[1]
code = "import 'dart:async';\nimport 'dart:io';\nimport 'dart:typed_data';\n"
for name in ['vault_keys', 'file_vault']:
    code += f"import '{(APP / 'lib/crypto' / (name + '.dart')).as_uri()}';\n"
code += '''
class ChangeNotifier {void notifyListeners(){} }
class DeviceVaultBackend extends MemoryVaultBackend {}
class VaultBytesCache {static void clear(){} }
class VideoFrames {static void clear(){} }
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
code += re.sub(r'^import .*;\n', '', (APP / 'lib/services/vault_service.dart').read_text(), flags=re.M)
tests = (APP / 'test/r13_lifecycle_cases.dart.inc').read_text()
# Сериализация БД требует освободить старую операцию ДО ожидания нового DB-open.
# Assertions сохранены: старое открытие отменено, новое открыто, поздний откат его не закрывает.
tests = tests.replace(
    "await s.lock();final next=error(s._open(await keys()));await StorageService.reached[1].future;",
    "await s.lock();final next=error(s._open(await keys()));StorageService.release[0].complete();await StorageService.reached[1].future;")
tests = tests.replace("StorageService.release[0].complete();check(await old!=null", "check(await old!=null")
code += tests
with tempfile.TemporaryDirectory(prefix='r14_lifecycle_') as directory:
    probe = Path(directory) / 'probe.dart'
    probe.write_text(code)
    raise SystemExit(subprocess.run([
        '/home/iam/.local/flutter-sdk/bin/cache/dart-sdk/bin/dart',
        f'--packages={APP / ".dart_tool/package_config.json"}', str(probe)], cwd=APP).returncode)
