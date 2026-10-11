Future<Directory> getApplicationDocumentsDirectory()async=>Directory(tmp);
class ConflictAlgorithm {static const replace='replace',ignore='ignore';}
class Sqflite {static int? firstIntValue(List<Map<String,Object?>> r)=>r.isEmpty?null:r.first.values.first as int;}
class Database {
 Database(this.path);final String path;
 Future<dynamic> sql(String sql,[List<dynamic> args=const[],bool query=false])async{
  final proc=await Process.start('python3',[bridge,path]);proc.stdin.write(jsonEncode({'sql':sql,'args':args,'query':query}));await proc.stdin.close();
  final out=await proc.stdout.transform(utf8.decoder).join(),err=await proc.stderr.transform(utf8.decoder).join();
  if(await proc.exitCode!=0)throw StateError(err);return jsonDecode(out);
 }
 Future<void> execute(String s)async{await sql(s);}
 Future<int> insert(String t,Map<String,dynamic> r,{String? conflictAlgorithm})async=>await sql('INSERT OR ${conflictAlgorithm=='ignore'?'IGNORE':'REPLACE'} INTO $t (${r.keys.join(',')}) VALUES (${List.filled(r.length,'?').join(',')})',r.values.toList());
 Future<int> delete(String t,{String? where,List<dynamic>? whereArgs})async=>await sql('DELETE FROM $t${where==null?'':' WHERE $where'}',whereArgs??[]);
 Future<int> update(String t,Map<String,dynamic> r,{String? where,List<dynamic>? whereArgs})async=>await sql('UPDATE $t SET ${r.keys.map((k)=>'$k=?').join(',')}${where==null?'':' WHERE $where'}',[...r.values,...?whereArgs]);
 Future<List<Map<String,Object?>>> query(String t,{List<String>? columns})async=>rawQuery('SELECT ${columns?.join(',')??'*'} FROM $t');
 Future<List<Map<String,Object?>>> rawQuery(String s,[List<dynamic> args=const[]])async=>(await sql(s,args,true) as List).map((r)=>(r as Map).cast<String,Object?>()).toList();
 Batch batch()=>Batch(this);Future<void> close()async{}
}
class Batch {
 Batch(this.d);final Database d;final List<Future<void> Function()> ops=[];
 void insert(String t,Map<String,dynamic> r,{String? conflictAlgorithm}){ops.add(()async{await d.insert(t,r,conflictAlgorithm:conflictAlgorithm);});}
 Future<void> commit({bool? noResult})async{for(final op in ops)await op();}
}
Future<Database> openDatabase(String path,{int? version,bool? readOnly,String? password,Future<void> Function(Database,int)? onCreate,Future<void> Function(Database,int,int)? onUpgrade})async{
 final exists=File(path).existsSync();final d=Database(path);
 if(!exists&&onCreate!=null){await onCreate(d,version!);await d.execute('PRAGMA user_version=$version');}
 else if(version!=null){final v=Sqflite.firstIntValue(await d.rawQuery('PRAGMA user_version'))??0;if(v<version){await onUpgrade?.call(d,v,version);await d.execute('PRAGMA user_version=$version');}}
 return d;
}
