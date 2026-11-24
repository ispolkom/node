import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import '../lib/crypto/vault_keys.dart';
import '../lib/crypto/file_vault.dart';
import 'bug_r14_repeat_round13_legacy_file_vault.dart' as old;
int passed=0,failed=0;
void check(bool b,String m){if(!b)throw StateError(m);}
bool eq(List<int>a,List<int>b)=>a.length==b.length&&List.generate(a.length,(i)=>a[i]==b[i]).every((x)=>x);
Future<Object?> error(Future<dynamic> f)async{try{await f;return null;}catch(e){return e;}}
Future<void> test(String n,Future<void> Function() f)async{try{await f();passed++;print('PASS $n');}catch(e){failed++;print('FAIL $n: $e');}}
const words=['alpha','bravo','charlie','delta'];
Future<VaultKeys> make(MemoryVaultBackend b,{bool legacy=false,bool phrase=true})=>VaultKeys.create(b,password:'old-password',phraseFields:phrase?words:null,pwKdf:VaultKdf.test,phKdf:VaultKdf.test,legacyPhraseEncoding:legacy);
class FaultBackend extends MemoryVaultBackend {
 String? failWrite,holdRead;
 Completer<void>? reached,release;
 @override Future<void> secureWrite(String key,String value)async{if(key==failWrite){failWrite=null;throw StateError('injected write failure $key');}await super.secureWrite(key,value);}
 @override Future<String?> secureRead(String key)async{if(key==holdRead){holdRead=null;reached!.complete();await release!.future;}return super.secureRead(key);}
}
Future<void> main()async{
 await test('r13_generation_crash_between_blob_and_marker_preserves_new_password',()async{
  final b=FaultBackend();final k=await make(b);final dk=await k.subKey('db');b.failWrite='vault_gen';
  check(await error(k.changePassword('old-password','new-password'))!=null,'fault missing');k.lock();
  check(eq(await(await VaultKeys.unlock(b,'new-password')).subKey('db'),dk),'newer blob rejected/lost DK');
  check(b.secure['vault_gen']=='2','generation not healed');
 });
 await test('r13_legacy_generation_and_phrase_upgrade_preserve_key',()async{
  final b=MemoryVaultBackend();final k=await make(b,legacy:true);final dk=await k.subKey('db');k.lock();
  final j=jsonDecode(b.blob!);j.remove('gen');b.blob=jsonEncode(j);for(final n in ['vault_gen','vault_cnt','vault_fail_pw','vault_fail_ph'])b.secure.remove(n);
  final u=await VaultKeys.unlock(b,'old-password');check(eq(await u.subKey('db'),dk),'legacy unlock');u.lock();
  final r=await VaultKeys.recoverWithPhrase(b,words,'new-password');check(eq(await r.subKey('db'),dk),'legacy words');r.lock();
  check(eq(await(await VaultKeys.unlock(b,'new-password')).subKey('db'),dk),'new password');
 });
 await test('r13_serial_queue_continues_after_backend_exception',()async{
  final b=FaultBackend();(await make(b)).lock();b.failWrite='vault_fail_pw';
  check(await error(VaultKeys.unlock(b,'old-password'))!=null,'fault missing');
  final k=await VaultKeys.unlock(b,'old-password').timeout(const Duration(seconds:5));check(k.unlocked,'queue poisoned');k.lock();
 });
 await test('r13_new_phrase_encoding_rejects_positional_collision',()async{
  final b=MemoryVaultBackend();final a=['alpha|bravo','charlie','delta','echoo'];final c=['alpha','bravo|charlie','delta','echoo'];
  (await VaultKeys.create(b,password:'old-password',phraseFields:a,pwKdf:VaultKdf.test,phKdf:VaultKdf.test)).lock();
  check(!VaultKeys.samePhrase(a,c),'UI same');check(await error(VaultKeys.recoverWithPhrase(b,c,'new-password')) is VaultWrongSecret,'collision accepted');
  (await VaultKeys.recoverWithPhrase(b,a,'new-password')).lock();
 });
 await test('bug_r13_missing_counters_must_gate_before_kdf',()async{
  final b=MemoryVaultBackend();(await make(b)).lock();for(var i=0;i<4;i++)await error(VaultKeys.unlock(b,'wrong-password'));
  b.secure.remove('vault_fail_pw');b.secure.remove('vault_next_pw');
  check(await error(VaultKeys.unlock(b,'old-password')) is VaultThrottled,'correct password runs KDF and resets all counters after deletion');
 });
 await test('bug_r13_password_change_during_lock_must_not_wrap_zero_data_key',()async{
  final b=FaultBackend();final k=await make(b,phrase:false);final dk=await k.subKey('db');
  // The old-password verification has one hw read; pause the second read, inside _setPassword.
  var reads=0;final proxy=CountingBackend(b,()=>++reads==2);final live=await VaultKeys.unlock(proxy,'old-password');reads=0;
  final changing=live.changePassword('old-password','new-password');await proxy.reached.future;live.lock();proxy.release.complete();
  final e=await error(changing);print('  change outcome=$e');
  final current=await VaultKeys.unlock(b,e==null?'new-password':'old-password');
  check(eq(await current.subKey('db'),dk),'password change committed a wrap of zeroed DK; no recovery phrase');
 });
 final tmp=Directory.systemTemp.createTempSync('r14_core-');
 try{
  final root=List.generate(32,(i)=>i+1);
  await test('bug_r13_pre_upgrade_v1_file_must_remain_readable',()async{
   final f=File('${tmp.path}/legacy.yfe');final v1=old.FileVault.withKey(root);final w=await v1.create(f,name:'owner.txt',mime:'text/plain',size:6);await w.add([1,2,3,4,5,6]);await w.close();
   check(eq(await v1.readAll(f),[1,2,3,4,5,6]),'legacy fixture');
   check(eq(await FileVault.withKey(root).readAll(f),[1,2,3,4,5,6]),'v1 no longer readable');
  });
  await test('r13_v2_tampering_is_rejected_under_same_storage_id',()async{
   final v=FileVault.withKey(root,chunkSize:1024);final f=File('${tmp.path}/same.yfe');final w=await v.create(f,name:'file',mime:'text/plain',size:3500);await w.add(List.generate(3500,(i)=>i%251));await w.close();final raw=await f.readAsBytes();final start=34+ByteData.sublistView(raw).getUint16(32);
   check((await v.readAll(f)).length==3500,'unmodified control');
   for(final pos in [0,4,5,8,24,28,33,40,start,start+1024,raw.length-1]){final bad=Uint8List.fromList(raw);bad[pos]^=1;await f.writeAsBytes(bad);check(await error(v.readAll(f))!=null,'tamper $pos accepted');}
   for(final bad in [raw.sublist(0,raw.length-1),[...raw,0]]){await f.writeAsBytes(bad);check(await error(v.readAll(f))!=null,'length tamper accepted');}
   final swap=Uint8List.fromList(raw);swap.setRange(start,start+1040,raw.sublist(start+1040,start+2080));swap.setRange(start+1040,start+2080,raw.sublist(start,start+1040));await f.writeAsBytes(swap);check(await error(v.readAll(f))!=null,'chunk swap accepted');
  });
  await test('r13_part_rename_directory_move_and_dispose_revoke_writer',()async{
   final v=FileVault.withKey(root);final part=File('${tmp.path}/transfer.yfe.part');final w=await v.create(part,name:'file',mime:'text/plain',size:6);await w.add([1,2,3,4,5,6]);await w.close();
   final dir=Directory('${tmp.path}/moved')..createSync();final target=await part.rename('${dir.path}/transfer.yfe');check(eq(await v.readAll(target),[1,2,3,4,5,6]),'part rename or directory move broken');
   final unfinished=File('${tmp.path}/pending.yfe.part');final pending=await v.create(unfinished,name:'file',mime:'text/plain',size:6);await pending.add([1,2,3]);v.dispose();
   check(await error(pending.close())!=null,'writer completed after dispose');check(!unfinished.existsSync(),'aborted part remains');
  });
 }finally{await tmp.delete(recursive:true);}
 print('RESULT $passed PASS / $failed FAIL');if(failed>0)exitCode=1;
}
class CountingBackend implements VaultBackend {
 final FaultBackend b;final bool Function() shouldHold;final reached=Completer<void>(),release=Completer<void>();
 CountingBackend(this.b,this.shouldHold);
 @override Future<String?> secureRead(String k)async{if(k=='vault_hw'&&shouldHold()){reached.complete();await release.future;}return b.secureRead(k);}
 @override Future<void> secureWrite(String k,String v)=>b.secureWrite(k,v);
 @override Future<void> secureDelete(String k)=>b.secureDelete(k);
 @override Future<String?> readBlob()=>b.readBlob();
 @override Future<void> writeBlob(String t)=>b.writeBlob(t);
 @override int nowMs()=>b.nowMs();
}
