int good=0,bad=0;
void check(bool b,String m){if(!b)throw StateError(m);}
Future<Object?> error(Future<dynamic> f)async{try{await f;return null;}catch(e){return e;}}
Future<void> test(String n,Future<void> Function() f)async{StorageService.reset();try{await f();good++;print('PASS $n');}catch(e){bad++;print('FAIL $n: $e');}}
Future<VaultKeys> keys()async=>VaultKeys.create(MemoryVaultBackend(),password:'123456',pwKdf:VaultKdf.test);
class DelayedBackend extends MemoryVaultBackend {
 bool hold=false;final reached=Completer<void>(),release=Completer<void>();
 @override Future<String?> secureRead(String k)async{if(hold&&k=='vault_hw'){hold=false;reached.complete();await release.future;}return super.secureRead(k);}
}

class DelayedKeys implements VaultKeys {
 final VaultKeys real;final reached=Completer<void>(),release=Completer<void>();
 DelayedKeys(this.real);
 @override bool get unlocked=>real.unlocked;
 @override void lock()=>real.lock();
 @override Future<Uint8List> subKey(String purpose)async{final k=await real.subKey(purpose);if(purpose=='files'){reached.complete();await release.future;}return k;}
 @override dynamic noSuchMethod(Invocation i)=>super.noSuchMethod(i);
}

Future<void> main()async{
 await test('r13_cancel_open_at_database_stage_cleans_state_and_can_retry',()async{
  final s=VaultService._();StorageService.gate();final k=await keys();final result=error(s._open(k));await StorageService.reached[0].future;await s.lock();StorageService.release[0].complete();check(await result!=null,'cancel succeeded');
  check(!s.active&&!s.unlocked&&!k.unlocked&&!StorageService.opened&&StorageService.secure==null,'cancel leaked state');
  await s._open(await keys());check(s.unlocked&&StorageService.opened,'retry failed');await s.lock();
 });
 await test('r13_failed_open_clears_opening_files_and_allows_retry',()async{
  final s=VaultService._();StorageService.failWrap=true;final k=await keys();check(await error(s._open(k))!=null,'no failure');
  check(!s.active&&s._filesPending==null&&!k.unlocked&&StorageService.secure==null,'failed state');
  check(await error(s.files())!=null,'files when failed');check(s._filesPending==null,'files pending forgotten');
  StorageService.failWrap=false;await s._open(await keys());final files=await Future.wait([s.files(),s.files()]);check(identical(files[0],files[1]),'two roots');await s.lock();check(files[0].disposed,'not revoked');
 });
 await test('r13_close_exception_still_revokes_keys_and_files',()async{
  final s=VaultService._();final k=await keys();await s._open(k);final v=await s.files();StorageService.failClose=true;await s.lock();check(!s.active&&!k.unlocked&&v.disposed&&StorageService.secure==null,'close failure leaked session');
 });
 await test('bug_r13_lock_during_password_derivation_must_cancel_unlock',()async{
  final s=VaultService._();final b=DelayedBackend();(await VaultKeys.create(b,password:'123456',pwKdf:VaultKdf.test)).lock();s.backend=b;b.hold=true;
  final result=error(s.unlock('123456'));await b.reached.future;print('  active during password verification=${s.active}');await s.lock();b.release.complete();final e=await result;
  check(!s.unlocked&&!StorageService.opened,'unlock begun before lock resurrected keys/database; result=$e');await s.lock();
 });
 await test('bug_r13_cancelled_old_open_must_not_close_new_session_database',()async{
  final s=VaultService._();StorageService.gate();StorageService.gate();final old=error(s._open(await keys()));await StorageService.reached[0].future;
  await s.lock();final next=error(s._open(await keys()));await StorageService.reached[1].future;StorageService.release[1].complete();check(await next==null&&s.unlocked&&StorageService.opened,'new session did not open');
  StorageService.release[0].complete();check(await old!=null,'old did not cancel');
  check(s.unlocked&&StorageService.opened,'late rollback of old generation closed current database');await s.lock();
 });
 await test('bug_r13_reopening_live_service_must_revoke_previous_keys_and_files',()async{
  final s=VaultService._();final b=MemoryVaultBackend();final k1=await VaultKeys.create(b,password:'123456',pwKdf:VaultKdf.test);await s._open(k1);final f1=await s.files();final k2=await VaultKeys.unlock(b,'123456');await s._open(k2);final f2=await s.files();await s.lock();
  check(!k1.unlocked&&f1.disposed&&!identical(f1,f2),'previous keys were abandoned alive / files from old generation reused');
 });

 await test('bug_r13_old_files_completion_must_not_clear_new_pending_future',()async{
  final s=VaultService._();final k1=DelayedKeys(await keys());await s._open(k1);final old=error(s.files());await k1.reached.future;await s.lock();
  final k2=DelayedKeys(await keys());await s._open(k2);final next=s.files();await k2.reached.future;final tracked=s._filesPending;
  k1.release.complete();check(await old!=null,'old not cancelled');final stayed=identical(s._filesPending,tracked);k2.release.complete();await next;await s.lock();
  check(stayed,'old whenComplete nulled the future belonging to new generation');
 });
 print('RESULT $good PASS / $bad FAIL');if(bad>0)exitCode=1;
}
