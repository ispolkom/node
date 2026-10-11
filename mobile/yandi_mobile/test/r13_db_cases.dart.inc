int good=0,bad=0;
void check(bool b,String m){if(!b)throw StateError(m);}
Future<Object?> error(Future<dynamic> f)async{try{await f;return null;}catch(e){return e;}}
Future<void> test(String n,Future<void> Function() f)async{try{await f();good++;print('PASS $n');}catch(e){bad++;print('FAIL $n: $e');}}
Future<void> clean()async{await StorageService.closeDb();StorageService.wipeHook=null;docsOverride=null;for(final f in Directory(tmp).listSync()){if(f is File&&f.path!=bridge)f.deleteSync();}}
Future<void> main()async{
 await test('r13_legacy_v1_schema_token_and_unverified_keys_migration',()async{
  final d=Database('$tmp/yandi.db');await d.execute('CREATE TABLE nodes (id TEXT PRIMARY KEY,name TEXT,host TEXT,port INTEGER,fingerprint TEXT,token TEXT,added_at INTEGER)');
  await d.insert('nodes',{'id':'peer','name':'owner','host':'host','port':1234,'fingerprint':'f','token':'secret','added_at':1});
  await d.execute('CREATE TABLE messages (id TEXT PRIMARY KEY,peer_id TEXT,outgoing INTEGER,text TEXT,timestamp INTEGER)');await d.insert('messages',{'id':'old','peer_id':'peer','outgoing':0,'text':'private','timestamp':1});await d.execute('PRAGMA user_version=1');
  await StorageService.openEncrypted(List.filled(32,1));check((await StorageService.db.query('messages')).single['id']=='old','messages lost');final node=(await StorageService.db.query('nodes')).single;
  check(node['version']==''&&node['token']==null,'upgrade missing');check((await StorageService.db.query('group_entries')).isEmpty,'group schema');
 });await clean();
 await test('bug_r13_migration_read_error_must_not_silently_wipe_source',()async{
  final d=await openDatabase('$tmp/yandi.db',version:5,onCreate:StorageService._createTables);await d.insert('messages',{'id':'precious','peer_id':'peer','outgoing':0,'text':'private','timestamp':1});
  failQueryTable='messages';final e=await error(StorageService.openEncrypted(List.filled(32,1)));
  check(e!=null&&File('$tmp/yandi.db').existsSync(),'query I/O failure treated as absent table; source removed and destination has no messages');
 });await clean();
 await test('r13_erase_plain_refusal_preserves_file_and_reports_failure',()async{
  final d=Directory('$tmp/erase-plain')..createSync();docsOverride=d;final f=File('${d.path}/plain.txt')..writeAsStringSync('private');StorageService.wipeHook=(_)async=>false;
  final left=await StorageService.eraseAllLocalData();check(f.existsSync()&&left!=null,'plain refusal not preserved');
 });await clean();
 await test('bug_r13_erase_legacy_plain_ledger_must_not_classify_as_encrypted',()async{
  final d=Directory('$tmp/erase-ledger')..createSync();docsOverride=d;final f=File('${d.path}/e2e_replay_e4.json')..writeAsStringSync('{"private-peer":999999}');StorageService.wipeHook=(_)async=>false;
  final left=await StorageService.eraseAllLocalData();check(f.existsSync()&&left!=null,'legacy plaintext ledger unlinked after refused wipe; erase reported success');
 });await clean();
 await test('bug_r13_erase_plain_wipe_suffix_must_not_classify_as_encrypted',()async{
  final d=Directory('$tmp/erase-wipe')..createSync();docsOverride=d;final f=File('${d.path}/old-private.txt.wipe')..writeAsStringSync('private');StorageService.wipeHook=(_)async=>false;
  final left=await StorageService.eraseAllLocalData();check(f.existsSync()&&left!=null,'plaintext .wipe treated as ciphertext and unlinked');
 });await clean();
 // Имя и короткий заголовок не доказывают, что всё содержимое зашифровано.
 await test('r13_erase_unverified_yfe_preserved_on_wipe_refusal',()async{
  final d=Directory('$tmp/erase-enc')..createSync();docsOverride=d;final f=File('${d.path}/file.yfe')..writeAsBytesSync([89,70,69,49,2]);StorageService.wipeHook=(_)async=>false;
  final left=await StorageService.eraseAllLocalData();check(f.existsSync()&&left!=null,'unverified yfe silently unlinked');
 });await clean();
 print('RESULT $good PASS / $bad FAIL');if(bad>0)exitCode=1;
}
