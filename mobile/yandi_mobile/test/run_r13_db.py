"""Run actual migration bodies using real Python SQLite on temporary databases.
SQLCipher password/native plugin is deliberately NOT emulated or tested.
"""
import json,re,subprocess,tempfile
from pathlib import Path
APP=Path(__file__).resolve().parents[1]
s=(APP/'lib/services/storage_service.dart').read_text()
with tempfile.TemporaryDirectory(prefix='r14_db-') as td:
 t=Path(td);bridge=t/'sql.py'
 bridge.write_text("""import sys,json,sqlite3
r=json.load(sys.stdin);db=sqlite3.connect(sys.argv[1]);db.row_factory=sqlite3.Row
c=db.execute(r['sql'],r.get('args',[]));out=[dict(x) for x in c.fetchall()] if r.get('query') else c.rowcount
db.commit();db.close();print(json.dumps(out))
""")
 code="import 'dart:io';\nimport 'dart:convert';\nimport 'package:path/path.dart' as p;\n"+f'const tmp={json.dumps(td)},bridge={json.dumps(str(bridge))};\n'
 adapter=(APP/'test/r13_db_adapter.dart.inc').read_text()
 adapter=adapter.replace('Future<Directory> getApplicationDocumentsDirectory()async=>Directory(tmp);','Directory? docsOverride; Future<Directory> getApplicationDocumentsDirectory()async=>docsOverride??Directory(tmp); Future<Directory> getApplicationSupportDirectory()async=>Directory(\"${docsOverride!.path}/support\"); Future<Directory> getTemporaryDirectory()async=>Directory(\"${docsOverride!.path}/cache\");')
 adapter=adapter.replace('Future<List<Map<String,Object?>>> query(String t,{List<String>? columns})async=>rawQuery', 'Future<List<Map<String,Object?>>> query(String t,{List<String>? columns})async{if(t==failQueryTable){failQueryTable=null;throw StateError(\"injected read I/O error\");}return rawQuery')
 adapter=adapter.replace("FROM $t');\n Future<List", "FROM $t');}\n Future<List")
 code+='String? failQueryTable;\n'+adapter
 code+='class RawSecure {Future<void> deleteAll()async{}}\n'

 code+='class StorageService { static Database? _db;static Object? replayLedger;static final _secure=RawSecure();static Object? _secureCache;static void setSecureKey(Object? k){} static const _nodeTokKey="node_token_"; static Future<void> secureWrite(String key,String value)async{}\n'
 code+=s[s.index('  static Future<String?> eraseAllLocalData()'):s.index('  static bool get dbOpen')]
 code+=s[s.index('  static bool get dbOpen'):s.index('  // ── Secure storage helpers')].replace('@visibleForTesting','')+ '\n}\n'
 code+=(APP/'test/r13_db_cases.dart.inc').read_text()
 f=t/'probe.dart';f.write_text(code)
 raise SystemExit(subprocess.run(['/home/iam/.local/flutter-sdk/bin/cache/dart-sdk/bin/dart',f'--packages={APP / ".dart_tool/package_config.json"}',str(f)],cwd=APP).returncode)
