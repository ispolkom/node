int good=0,bad=0;
void check(bool b,String m){if(!b)throw StateError(m);}
Future<Object?> error(Future<dynamic> f)async{try{await f;return null;}catch(e){return e;}}
Future<void> test(String n,Future<void> Function() f)async{StorageService._secure.kv.clear();StorageService._secureCache=null;StorageService.setSecureKey(List.filled(32,7));try{await f();good++;print('PASS $n');}catch(e){bad++;print('FAIL $n: $e');}}
Future<void> main()async{
 final tmp=Directory.systemTemp.createTempSync('r14_service-');
 try{
  await test('r13_storage_owned_secure_key_is_erased_input_key_is_copied',()async{
   final input=Uint8List.fromList(List.filled(32,7));StorageService.setSecureKey(input);final retained=StorageService._secureKey!;StorageService.setSecureKey(null);
   check(retained.every((x)=>x==0),'owned key not erased');check(input.every((x)=>x==7),'input unexpectedly mutated');
  });
  await test('r13_ledger_first_run_reseal_marker_and_restart_reject_downgrade',()async{
   final f=File('${tmp.path}/ledger');final l=ReplayLedger(f,seal:StorageService.ledgerSeal,open:StorageService.ledgerOpen,afterWrite:StorageService.ledgerSealed);
   await l.reseal();check(!f.existsSync()&&await StorageService.secureRead('vault_ledger_sealed')==null,'first run blocked');
   f.writeAsStringSync('{"old":5000}');await l.reseal();check(f.readAsStringSync().startsWith('e1:')&&await StorageService.secureRead('vault_ledger_sealed')=='1','reseal order');
   check(!await l.claim('old',5000,nowMs:1000),'old ID lost');StorageService.dropSecureCache();f.writeAsStringSync('{}');check(await error(l.claim('old',5000,nowMs:1000))!=null,'plain downgrade');
  });
  await test('bug_r13_missing_sealed_ledger_must_fail_closed',()async{
   final f=File('${tmp.path}/ledger-deleted');final l=ReplayLedger(f,seal:StorageService.ledgerSeal,open:StorageService.ledgerOpen,afterWrite:StorageService.ledgerSealed);
   await l.claim('accepted',5000,nowMs:1000);check(await StorageService.secureRead('vault_ledger_sealed')=='1','marker missing');await f.delete();
   check(await error(l.claim('accepted',5000,nowMs:1000))!=null,'deleted sealed ledger treated as first run; accepted ID replayed despite durable marker');
  });
  await test('bug_r13_ledger_marker_write_crash_must_not_leave_downgrade_window',()async{
   final f=File('${tmp.path}/ledger-fault');final l=ReplayLedger(f,seal:StorageService.ledgerSeal,open:StorageService.ledgerOpen,afterWrite:StorageService.ledgerSealed);
   f.writeAsStringSync('{"old":5000}');StorageService._secure.failWrite='vault_ledger_sealed';check(await error(l.reseal())!=null,'fault missing');check(f.readAsStringSync().startsWith('e1:'),'file not committed');
   // Перезапуск и reseal существующего контейнера должны восстановить недостающую метку.
   StorageService.dropSecureCache();await l.reseal();f.writeAsStringSync('{}');
   check(await error(l.claim('old',5000,nowMs:1000))!=null,'after encrypted-file commit and marker failure, restart still accepts plaintext replay');
  });
  await test('r13_real_resolveId_preserves_personal_and_group_file_lookup',()async{
   final v=FileVault.withKey(List.filled(32,7));final dir=Directory('${tmp.path}/migrate')..createSync();final s=AppState();s.vf=VaultFiles(dir,vault:()async=>v);
   final tid='abcdef0123456789abcdef0123456789';final path='${dir.path}/abcdef01_old.txt';File(path).writeAsStringSync('personal');
   final o=FileOffer(tid:tid,name:'old.txt',size:8,chunks:1,mime:'text/plain',key:base64.encode(List.filled(32,1)));
   StorageService.db.rows.clear();StorageService.db.rows.add({'text':o.encode()});
   final gp='${dir.path}/fedcba98_group.txt';File(gp).writeAsStringSync('group');final fid='fedcba9876543210fedcba9876543210';await StorageService.secureWrite('grp_dl_$fid',gp);
   check(await s.vf.migrateLegacy(resolveId:s._legacyIdFor)==2,'not migrated');check(File(s.vf.pathFor(tid)).existsSync()&&File(s.vf.pathFor(fid)).existsSync(),'lookup lost');
   check(await s.vf.migrateLegacy(resolveId:s._legacyIdFor)==0,'not idempotent');
  });
  await test('bug_r13_legacy_retry_must_verify_chunks_before_wiping_only_good_copy',()async{
   final v=FileVault.withKey(List.filled(32,7));final dir=Directory('${tmp.path}/retry')..createSync();final old=File('${dir.path}/abcdef01_old.txt')..writeAsStringSync('precious');
   var allow=false;final vf=VaultFiles(dir,vault:()async=>v,wipe:(p)async{if(!allow)return false;await File(p).delete();return true;});
   await vf.migrateLegacy();final enc=dir.listSync().whereType<File>().singleWhere((x)=>x.path.endsWith('.yfe'));final bytes=enc.readAsBytesSync();bytes[bytes.length-1]^=1;enc.writeAsBytesSync(bytes);allow=true;
   await vf.migrateLegacy();check(old.existsSync()||await error(v.readAll(enc))==null,'readMeta/size passed but corrupt chunks; last valid plaintext copy wiped');
  });
  await test('r13_picker_cleanup_success_and_migration_warning_are_reported',()async{
   pp.cache=Directory('${tmp.path}/cache')..createSync();final picker=Directory('${pp.cache.path}/file_picker')..createSync();final f=File('${picker.path}/chosen')..writeAsStringSync('private');
   AppState.hook=(p)async{await File(p).delete();return true;};final s=AppState();final dir=Directory('${tmp.path}/housekeeping')..createSync();s.vf=VaultFiles(dir,vault:()async=>FileVault.withKey(List.filled(32,7)),wipe:(_)async=>false);
   File('${dir.path}/legacy.txt').writeAsStringSync('private');await s._filesHousekeeping();check(!f.existsSync(),'picker not cleaned');check(s.protectionWarning!=null,'legacy wipe refusal not reported');
  });
  await test('bug_r13_failed_picker_wipe_must_set_protection_warning',()async{
   final picker=Directory('${pp.cache.path}/file_picker');final f=File('${picker.path}/chosen')..writeAsStringSync('private');AppState.hook=(_)async=>false;
   final s=AppState();s.vf=VaultFiles(Directory('${tmp.path}/empty')..createSync(),vault:()async=>FileVault.withKey(List.filled(32,7)));await s._filesHousekeeping();
   check(f.existsSync(),'plain unlinked');check(s.protectionWarning!=null,'plaintext picker copy remains with no warning');
  });
  await test('bug_r13_housekeeping_aggregates_temp_and_database_wipe_failures',()async{
   AppState.hook=(p)async{await File(p).delete();return true;};
   final dir=Directory('${tmp.path}/cleanup-count')..createSync();
   final vf=VaultFiles(dir,vault:()async=>FileVault.withKey(List.filled(32,7)),wipe:(_)async=>false);
   final temporary=File('${dir.path}/temporary.txt')..writeAsStringSync('private');
   // cleanTemp() обрабатывает стандартный подкаталог временных копий.
   final tempDir=Directory('${dir.path}/tmp')..createSync();
   temporary.renameSync('${tempDir.path}/temporary.txt');
   final s=AppState()..vf=vf;StorageService.cleanupFailures=1;
   await s._filesHousekeeping();
   check(vf.cleanupFailures>0&&File('${tempDir.path}/temporary.txt').existsSync(),'temp cleanup refusal not counted/preserved');
   check(s.protectionWarning!=null,'database cleanup failure hidden');
   StorageService.cleanupFailures=0;
  });
  await test('r14_picker_sweep_and_cleanup_do_not_wipe_outside_cache',()async{
   final cache=Directory('${tmp.path}/safe-cache')..createSync();pp.cache=cache;
   final outside=Directory('${tmp.path}/safe-cache-other')..createSync();
   final precious=File('${outside.path}/private.txt')..writeAsStringSync('must stay');
   final touched=<String>[];AppState.hook=(p)async{touched.add(p);await File(p).delete();return true;};
   final s=AppState();await s.dropPickerCopy(precious.path);
   Link('${cache.path}/file_picker').createSync(outside.path);
   await s._sweepPickerCache();
   check(touched.isEmpty&&precious.existsSync(),'sweep followed picker directory outside cache');
   check(s.protectionWarning!=null,'unsafe picker directory not reported');
  });
 }finally{await tmp.delete(recursive:true);}
 print('RESULT $good PASS / $bad FAIL');if(bad>0)exitCode=1;
}
