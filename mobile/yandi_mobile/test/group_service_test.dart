import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/group_crypto.dart';
import 'package:yandi_mobile/models/group.dart';
import 'dart:io';
import 'package:yandi_mobile/services/group_api.dart';
import 'package:yandi_mobile/services/group_files.dart';
import 'package:yandi_mobile/services/group_service.dart';
import 'package:yandi_mobile/services/group_store.dart';
import 'package:yandi_mobile/services/group_store_sqlite.dart' show secureKeyOfGroup;

/// Узел в миниатюре: хранит непрозрачные байты, как настоящий (проверка ролей — упрощённая).
class FakeNode {
  final String gid = 'ab' * 16;
  int epoch = 0;
  bool rekeyNeeded = false;
  final members = <String, Map<String, dynamic>>{};     // device → {alias, role, muted, pub}
  final boxes = <int, Map<String, String>>{};            // epoch → alias → box
  final log = <Map<String, dynamic>>[];
  String? ownerAlias;
  final files = <String, Map<int, List<int>>>{};
  final reqs = <Map<String, dynamic>>[];       // просьбы вступить: {id, dev, name}
  final boxOwner = <int, String?>{};            // поколение → ключ владельца, запечатавшего конверты
  final deletions = <int>[];                    // номера стёртых записей по порядку
  String name = 'Тест', topic = '', about = '';
  final doneFiles = <String>{};

  FakeNode(String ownerDevice) {
    members[ownerDevice] = {'alias': 'Хозяин-1', 'role': 'owner', 'muted': false, 'pub': null};
    ownerAlias = 'Хозяин-1';
  }
  GroupApi as(String dev) => _Api(this, dev);
}

class _Api implements GroupApi {
  final FakeNode n;
  final String dev;
  _Api(this.n, this.dev);
  Map<String, dynamic> get me => n.members[dev] ?? (throw const GroupApiError(403, 'не участник'));

  @override
  Future<List<Map<String, dynamic>>> list() async {
    if (!n.members.containsKey(dev)) return [];
    final pending = [for (final m in n.members.values) if (m['pub'] != null && !(n.boxes[n.epoch]?.containsKey(m['alias']) ?? false) && m['role'] != 'owner') m['alias'] as String];
    return [
      {'id': n.gid, 'name': n.name, 'topic': n.topic, 'about': n.about, 'kind': 'invite', 'my_alias': me['alias'], 'role': me['role'], 'muted': me['muted'], 'members': n.members.length,
       'epoch': n.epoch, 'first_seq': 1, 'last_seq': n.log.length, 'rekey_needed': n.rekeyNeeded, 'pending_keys': pending}
    ];
  }
  @override
  Future<List<Map<String, dynamic>>> openGroups() async => n.members.containsKey(dev)
      ? []
      : [{'id': n.gid, 'name': 'Тест', 'topic': 'тема', 'kind': 'invite', 'members': n.members.length, 'requested': n.reqs.any((r) => r['dev'] == dev)}];
  @override
  Future<void> settings(String gid, {String? name, String? topic, String? about}) async {
    if (me['role'] != 'owner' && me['role'] != 'admin') throw const GroupApiError(403, 'настройки группы меняют владелец и админ');
    if (name != null && name.trim().isEmpty) throw const GroupApiError(400, 'нужно название группы');
    if (name != null) n.name = name;
    if (topic != null) n.topic = topic;
    if (about != null) n.about = about;
  }
  @override
  Future<Map<String, dynamic>> deleted(String gid, int after) async {
    me;
    return {'deleted': n.deletions.skip(after).toList(), 'cursor': n.deletions.length, 'resync': false};
  }
  @override
  Future<void> transferOwner(String gid, String alias) async {
    if (me['role'] != 'owner') throw const GroupApiError(403, 'передать группу может только её владелец');
    final target = n.members.values.firstWhere((m) => m['alias'] == alias);
    if (target['pub'] == null) throw const GroupApiError(409, 'нет ключа');
    me['role'] = 'admin';
    target['role'] = 'owner';
    n.rekeyNeeded = true;
  }
  @override
  Future<void> requestJoin(String gid, String name) async {
    if (n.reqs.any((r) => r['dev'] == dev)) throw const GroupApiError(409, 'просьба уже отправлена');
    n.reqs.add({'id': 'r${n.reqs.length}', 'dev': dev, 'name': name});
  }
  @override
  Future<List<Map<String, dynamic>>> requests(String gid) async {
    if (me['role'] == 'member') throw const GroupApiError(403, 'нет прав');
    return [for (final r in n.reqs) {'id': r['id'], 'peer_id': 'ab' * 32, 'name': r['name'], 'ts': 1}];
  }
  @override
  Future<void> resolveRequest(String gid, String rid) async {
    if (me['role'] == 'member') throw const GroupApiError(403, 'нет прав');
    n.reqs.removeWhere((r) => r['id'] == rid);
  }
  @override
  Future<String> create(String name, String topic, String about, String kind) async => n.gid;
  @override
  Future<String> join(String gid, {String? inviteCode, String? memberPub}) async {
    if (inviteCode != 'AAAA-BBBB-CCCC') throw const GroupApiError(403, 'неверное приглашение');
    final alias = 'Гость-${n.members.length}';
    n.members[dev] = {'alias': alias, 'role': 'member', 'muted': false, 'pub': memberPub};
    return alias;
  }
  @override
  Future<void> leave(String gid) async { n.members.remove(dev); n.rekeyNeeded = true; }
  @override
  Future<Map<String, dynamic>> members(String gid) async => {
        'members': [for (final m in n.members.values) {'alias': m['alias'], 'role': m['role'], 'muted': m['muted'], 'member_pub': m['pub'], 'has_key': n.boxes[n.epoch]?.containsKey(m['alias']) ?? false}]
      };
  @override
  Future<void> setPub(String gid, String memberPub) async => me['pub'] = memberPub;
  @override
  Future<Map<String, dynamic>> keys(String gid) async => {
        'epoch': n.epoch,
        'owner_pub': n.members.values.firstWhere((m) => m['role'] == 'owner')['pub'],
        'boxes': [for (final e in n.boxes.entries) if (e.value.containsKey(me['alias'])) {'epoch': e.key, 'box': e.value[me['alias']], 'owner_pub': n.boxOwner[e.key]}],
      };
  @override
  Future<void> putKeys(String gid, int epoch, Map<String, String> boxes) async {
    if (me['role'] != 'owner') throw const GroupApiError(403, 'только владелец');
    if (epoch > n.epoch) { n.epoch = epoch; n.rekeyNeeded = false; }
    n.boxOwner[epoch] = me['pub'] as String?;
    (n.boxes[epoch] ??= {}).addAll(boxes);
  }
  @override
  Future<Map<String, dynamic>> pull(String gid, int after, {int limit = 200}) async {
    me;
    return {'first_seq': 1, 'last_seq': n.log.length, 'entries': [for (final e in n.log) if ((e['seq'] as int) > after) e].take(limit).toList()};
  }
  @override
  Future<int> append(String gid, int epoch, String payloadB64) async {
    if (me['muted'] == true) throw const GroupApiError(403, 'заглушён');
    final seq = n.log.length + 1;
    n.log.add({'seq': seq, 'ts': 1000 + seq, 'from': me['alias'], 'epoch': epoch, 'payload_b64': payloadB64, 'deleted': false});
    return seq;
  }
  @override
  Future<void> mod(String gid, Map<String, dynamic> body) async {
    if (body['action'] == 'delete_entry') { final e = n.log[(body['seq'] as int) - 1]; e['payload_b64'] = ''; e['deleted'] = true; n.deletions.add(body['seq'] as int); }
    if (body['action'] == 'mute') n.members.values.firstWhere((m) => m['alias'] == body['alias'])['muted'] = true;
    if (const {'make_admin', 'promote', 'demote'}.contains(body['action'])) {
      n.members.values.firstWhere((m) => m['alias'] == body['alias'])['role'] = switch (body['action']) { 'make_admin' => 'admin', 'promote' => 'moderator', _ => 'member' };
    }
  }
  @override
  Future<Map<String, dynamic>> invite(String gid, {int uses = 1, int ttlHours = 24}) async => {'code': 'AAAA-BBBB-CCCC'};
  @override
  Future<List<Map<String, dynamic>>> modlog(String gid) async => [];
  @override
  Future<({String fileId, int chunkMax})> fileStart(String gid, int size, int chunks) async {
    me;
    final id = 'f${n.files.length}'.padRight(32, '0');
    n.files[id] = {};
    return (fileId: id, chunkMax: 1 << 20);
  }
  @override
  Future<void> filePut(String gid, String fileId, int idx, List<int> bytes) async { me; n.files[fileId]![idx] = bytes; }
  @override
  Future<void> fileDone(String gid, String fileId) async { me; n.doneFiles.add(fileId); }
  @override
  Future<List<int>> fileGet(String gid, String fileId, int idx) async {
    me;
    if (!n.doneFiles.contains(fileId)) throw const GroupApiError(404, 'нет');
    return n.files[fileId]![idx]!;
  }
  @override
  Future<void> fileDelete(String gid, String fileId) async { me; n.files.remove(fileId); n.doneFiles.remove(fileId); }
}

void main() {
  late FakeNode node;
  late GroupService owner, bob, eve;
  late MemoryGroupStore ownerStore, bobStore;

  setUp(() {
    node = FakeNode('owner');
    ownerStore = MemoryGroupStore();
    bobStore = MemoryGroupStore();
    owner = GroupService(node.as('owner'), ownerStore);
    bob = GroupService(node.as('bob'), bobStore);
    eve = GroupService(node.as('eve'), MemoryGroupStore());
  });

  Future<void> bobJoins() async {
    await bob.join(node.gid, inviteCode: 'AAAA-BBBB-CCCC', displayName: 'Боб');
    await owner.sync();                 // владелец видит ключ Боба и раздаёт группу
    await bob.syncAndAnnounce();        // Боб забирает ключ, пишет имя
  }

  test('кривые данные узла и подлинные записи с неверными типами не ломают ленту', () async {
    await owner.sync();
    await bobJoins();
    // запись с неверным номером поколения, затем подлинная с текстом не-строкой, затем нормальная
    node.log.add({'seq': node.log.length + 1, 'ts': 1, 'from': 'Хозяин-1', 'epoch': 'wrong', 'payload_b64': 'AAAA', 'deleted': false});
    await owner.send(node.gid, GroupKind.text, {'t': {'nested': 1}});
    await owner.send(node.gid, GroupKind.text, {'t': 'после мусора'});
    await bob.sync();
    final texts = (await bob.messages(node.gid)).map((e) => e.body['t']).toList();
    expect(texts, ['после мусора'], reason: 'повреждённое помечено и скрыто, лента читается дальше');
    // список групп с одной кривой строкой: остальные группы на месте
    final list = GroupInfo.listOf([{'id': 42}, {'id': node.gid, 'name': 'Тест', 'topic': '', 'kind': 'invite', 'my_alias': 'x', 'role': 'member'}]);
    expect(list.map((g) => g.id), [node.gid]);
  });

  test('владелец раздаёт ключ, участник читает и пишет, имя доходит', () async {
    await owner.sync();                 // владелец создаёт первое поколение
    await bobJoins();
    await owner.send(node.gid, GroupKind.text, {'t': 'всем привет'});
    await bob.sync();
    final msgs = await bob.messages(node.gid);
    expect(msgs.map((e) => e.body['t']), ['всем привет']);
    await bob.send(node.gid, GroupKind.text, {'t': 'я тут'});
    await owner.sync();
    expect((await owner.messages(node.gid)).map((e) => e.body['t']), ['всем привет', 'я тут']);
    expect(owner.displayName(node.gid, 'Гость-1'), 'Боб');
    // узел хранит только шифртекст
    expect(node.log.every((e) => !utf8.decode(base64.decode(e['payload_b64'] as String), allowMalformed: true).contains('привет')), isTrue);
  });

  test('чужой без приглашения не входит; не участник ничего не читает', () async {
    await owner.sync();
    await expectLater(eve.join(node.gid, inviteCode: 'ЛЕВЫЙ', displayName: 'Ева'), throwsA(isA<GroupApiError>()));
    await eve.sync();
    expect(eve.groups, isEmpty);
  });

  test('запрет вида: отправитель отказывается, у получателей запись скрыта', () async {
    await owner.sync();
    await bobJoins();
    await owner.setRules(node.gid, const GroupRules({GroupKind.image, GroupKind.link}));
    await bob.sync();
    expect(bob.rules(node.gid).allows(GroupKind.image), isFalse);
    await expectLater(bob.send(node.gid, GroupKind.image, {'name': 'a.jpg'}), throwsA(isA<GroupSendRefused>()));
    // участник с «переделанным» клиентом всё же отправил — другим клиентам запись не показывается
    final g = bob.byId(node.gid)!;
    final key = (await bobStore.read('grp_key_${g.epoch}_${node.gid}'))!;
    final payload = await GroupCrypto.encryptEntry(base64.decode(key), gid: node.gid, epoch: g.epoch, from: g.myAlias, kind: GroupKind.image, body: {'name': 'x.jpg'});
    await node.as('bob').append(node.gid, g.epoch, base64.encode(payload));
    await owner.send(node.gid, GroupKind.text, {'t': 'ок'});
    await owner.sync();
    expect((await owner.messages(node.gid)).map((e) => e.kind), [GroupKind.text]);
  });

  test('правила принимаются только от владельца', () async {
    await owner.sync();
    await bobJoins();
    final g = bob.byId(node.gid)!;
    final key = base64.decode((await bobStore.read('grp_key_${g.epoch}_${node.gid}'))!);
    final payload = await GroupCrypto.encryptEntry(key, gid: node.gid, epoch: g.epoch, from: g.myAlias, kind: GroupKind.system, body: const GroupRules({GroupKind.text}).toBody());
    await node.as('bob').append(node.gid, g.epoch, base64.encode(payload));    // участник пытается запретить текст всем
    await owner.sync();
    await bob.sync();
    expect(owner.rules(node.gid).allows(GroupKind.text), isTrue);
    expect(bob.rules(node.gid).allows(GroupKind.text), isTrue);
    await expectLater(bob.setRules(node.gid, const GroupRules({GroupKind.text})), throwsA(isA<GroupSendRefused>()));
  });

  test('подмена ключа владельца узлом: ключи не принимаются', () async {
    await owner.sync();
    await bobJoins();
    await owner.send(node.gid, GroupKind.text, {'t': 'раз'});
    await bob.sync();
    node.members['owner']!['pub'] = base64.encode(List.filled(32, 7));   // узел подсунул чужой ключ владельца
    node.rekeyNeeded = true;
    await owner.sync();
    await bob.sync();
    expect(bob.hasProblem(node.gid), isTrue);
  });

  test('смена ключа после выхода участника: оставшиеся читают новое, вышедший — нет', () async {
    await owner.sync();
    await bobJoins();
    await bob.leave(node.gid);
    await owner.sync();                  // rekey: epoch растёт, Бобу конверта нет
    expect(node.epoch, 2);
    await owner.send(node.gid, GroupKind.text, {'t': 'без Боба'});
    await owner.sync();
    expect((await owner.messages(node.gid)).last.body['t'], 'без Боба');
    await bob.sync();
    expect(bob.groups, isEmpty);
  });

  test('запись стёрта модератором: остаётся пометка, текста нет', () async {
    await owner.sync();
    await bobJoins();
    final seq = await owner.send(node.gid, GroupKind.text, {'t': 'лишнее'});
    await bob.sync();
    expect((await bob.messages(node.gid)).length, 1);
    await owner.moderate(node.gid, 'delete_entry', seq: seq);
    await bob.sync();                    // уже скачано — стирание узнаём при полном обходе записи
    await bobStore.markDeleted(node.gid, seq);
    expect(await bob.messages(node.gid), isEmpty);
  });

  test('заглушённый не может писать', () async {
    await owner.sync();
    await bobJoins();
    await owner.moderate(node.gid, 'mute', alias: 'Гость-1');
    await bob.sync();
    await expectLater(bob.send(node.gid, GroupKind.text, {'t': 'эй'}), throwsA(isA<GroupSendRefused>()));
  });

  test('файл в группе: заливка, скачивание другим участником, подмена куска и чужой не читает', () async {
    await owner.sync();
    await bobJoins();
    final dir = Directory.systemTemp.createTempSync('grpfile');
    addTearDown(() => dir.deleteSync(recursive: true));
    final src = File('${dir.path}/a.bin')..writeAsBytesSync(List.generate(700 * 1024 + 123, (i) => (i * 7 + i ~/ 251) & 255));
    final up = GroupFiles(node.as('owner'));
    final d = await up.upload(node.gid, src, 'a.bin');
    expect(d['c'], 3);
    // узел видит только шифртекст
    final stored = node.files[d['f']]!.values.expand((x) => x).toList();
    expect(stored.length, greaterThan(src.lengthSync()));
    final out = File('${dir.path}/out.bin');
    await GroupFiles(node.as('bob')).download(node.gid, d, out);
    expect(out.readAsBytesSync(), src.readAsBytesSync());
    // чужой (не участник) не скачивает
    await expectLater(GroupFiles(node.as('eve')).download(node.gid, d, File('${dir.path}/e.bin')), throwsA(anything));
    // подмена куска узлом
    node.files[d['f']]![1] = [...node.files[d['f']]![1]!]..[20] ^= 1;
    final bad = File('${dir.path}/bad.bin');
    await expectLater(GroupFiles(node.as('bob')).download(node.gid, d, bad), throwsA(anything));
    expect(bad.existsSync(), isFalse, reason: 'повреждённый файл не остаётся');
    // перестановка кусков
    final f = node.files[d['f']]!;
    final a = f[0]!; f[0] = f[2]!; f[2] = a;
    await expectLater(GroupFiles(node.as('bob')).download(node.gid, d, bad), throwsA(anything));
  });

  test('вид файла по имени', () {
    expect(GroupFiles.kindForName('Фото.JPG'), GroupKind.image);
    expect(GroupFiles.kindForName('song.mp3'), GroupKind.audio);
    expect(GroupFiles.kindForName('clip.mp4'), GroupKind.video);
    expect(GroupFiles.kindForName('doc.pdf'), GroupKind.file);
    expect(GroupFiles.kindForName('noext'), GroupKind.file);
  });

  test('владелец пишет сразу после первой синхронизации (поколение в списке не отстаёт)', () async {
    await owner.sync();
    expect(owner.byId(node.gid)!.epoch, 1);
    await owner.send(node.gid, GroupKind.text, {'t': 'первое'});
  });

  test('создание группы телефоном: владелец сразу получает ключ', () async {
    final gid = await owner.createGroup('Новая', 'тема', '', 'invite');
    expect(gid, node.gid);
    expect(owner.byId(gid)!.epoch, 1);
  });

  test('группа удалена на узле: пропадает из списка, помечается «удалена», локальные данные чистятся по команде', () async {
    await owner.sync();
    await bobJoins();
    await bob.send(node.gid, GroupKind.text, {'t': 'привет'});
    await bob.sync();                                 // своя запись вернулась из журнала
    expect(bob.gone, isEmpty);
    node.members.remove('bob');                       // узел группу для Боба больше не показывает
    await bob.sync();
    expect(bob.groups, isEmpty);
    expect(bob.gone.map((g) => g.$1), [node.gid]);
    expect(bobStore.log[node.gid], isNotNull, reason: 'история пока на месте');
    await bob.forgetLocal(node.gid);
    expect(bob.gone, isEmpty);
    expect(bobStore.log[node.gid], isNull);
    expect(bobStore.kv.keys.where((k) => k.endsWith(node.gid)), isEmpty, reason: 'ключи группы стёрты');
    await bob.sync();
    expect(bob.gone, isEmpty, reason: 'забытая группа не возвращается');
  });

  test('просьба о приглашении: гость видит группу, просит, владелец видит и закрывает; рядовой участник не видит', () async {
    await owner.sync();
    final list = await eve.discover();
    expect(list.single.kind, 'invite');
    expect(list.single.requested, isFalse);
    await eve.requestInvite(node.gid, 'Ева');
    expect((await eve.discover()).single.requested, isTrue);
    await expectLater(eve.requestInvite(node.gid, 'Ева'), throwsA(isA<GroupApiError>()));
    final r = await owner.joinRequests(node.gid);
    expect(r.single.name, 'Ева');
    expect(r.single.peerId.length, 64);
    await bobJoins();
    await expectLater(bob.joinRequests(node.gid), throwsA(isA<GroupApiError>()), reason: 'рядовой участник просьб не видит');
    await owner.closeRequest(node.gid, r.single.id);
    expect(await owner.joinRequests(node.gid), isEmpty);
  });

  test('«больше не напоминать»: просьба закрывается, повторные просьбы этого человека не показываются', () async {
    await owner.sync();
    await eve.requestInvite(node.gid, 'Ева');
    final r = (await owner.joinRequests(node.gid)).single;
    await owner.ignoreRequester(node.gid, r);
    expect(await owner.joinRequests(node.gid), isEmpty);
    node.reqs.add({'id': 'again', 'dev': 'eve2', 'name': 'Ева снова'});     // просьба с тем же устройством приходит заново
    final same = {'id': 'x1', 'dev': 'eve', 'name': 'Ева'};
    node.reqs..clear()..add(same);
    expect(await owner.joinRequests(node.gid), isEmpty, reason: 'тихо закрыта');
    expect(node.reqs, isEmpty, reason: 'и убрана с узла');
  });

  test('карточка приглашения: использованный код помнится, для открытой группы (без кода) не блокируется', () async {
    expect(await bob.inviteSpent(node.gid, 'AAAA-BBBB-CCCC'), isFalse);
    await bob.markInviteSpent(node.gid, 'AAAA-BBBB-CCCC');
    expect(await bob.inviteSpent(node.gid, 'AAAA-BBBB-CCCC'), isTrue);
    expect(await bob.inviteSpent(node.gid, 'DDDD-EEEE-FFFF'), isFalse, reason: 'другой код');
    await bob.markInviteSpent(node.gid, '');
    expect(await bob.inviteSpent(node.gid, ''), isFalse, reason: 'открытая группа: можно вступить снова после выхода');
    await bob.markInviteSpent(node.gid, '', gone: true);
    expect(await bob.inviteSpent(node.gid, ''), isTrue, reason: 'группы нет (404) или вход запрещён: карточка гаснет и для открытой');
  });

  test('использованный код приглашения остаётся использованным и после выхода из группы', () async {
    await bob.markInviteSpent(node.gid, 'AAAA-BBBB-CCCC');
    await bob.store.forgetGroup(node.gid); // то же, что делает leave после ответа узла
    expect(await bob.inviteSpent(node.gid, 'AAAA-BBBB-CCCC'), isTrue);
    expect(secureKeyOfGroup('inv_spent_${node.gid}_AAAA-BBBB-CCCC', node.gid), isFalse, reason: 'отметка не среди записей группы');
  });

  test('настройки группы: владелец меняет название/тематику/описание, остальные нет, пустое название отвергается', () async {
    await owner.sync();
    await bobJoins();
    await owner.updateSettings(node.gid, name: 'Новое имя', topic: 'Садоводство', about: 'про грядки');
    expect(owner.byId(node.gid)!.name, 'Новое имя');
    expect(owner.byId(node.gid)!.topic, 'Садоводство');
    expect(owner.byId(node.gid)!.about, 'про грядки');
    await bob.sync();
    expect(bob.byId(node.gid)!.name, 'Новое имя', reason: 'у участников обновляется после синхронизации');
    await expectLater(bob.updateSettings(node.gid, name: 'Взлом'), throwsA(isA<GroupApiError>()));
    await expectLater(owner.updateSettings(node.gid, name: '  '), throwsA(isA<GroupApiError>()));
  });

  test('админ: владелец назначает, админ меняет настройки и задаёт правила, остальные клиенты принимают правила от админа', () async {
    await owner.sync();
    await bobJoins();
    await owner.moderate(node.gid, 'make_admin', alias: 'Гость-1');
    await bob.sync();
    expect(bob.byId(node.gid)!.isAdmin, isTrue);
    expect(bob.byId(node.gid)!.canManage, isTrue);
    await bob.updateSettings(node.gid, name: 'Имя от админа');
    await bob.setRules(node.gid, const GroupRules({GroupKind.video}));
    await owner.sync();
    expect(owner.rules(node.gid).allows(GroupKind.video), isFalse, reason: 'владелец принял правила от админа');
    expect(owner.byId(node.gid)!.name, 'Имя от админа');
    await eve.sync();
  });

  test('передача владения: новый владелец раздаёт новое поколение, участники принимают его ключ по записи изнутри группы', () async {
    await owner.sync();
    await bobJoins();                       // Боб — участник, закрепил ключ владельца
    await owner.send(node.gid, GroupKind.text, {'t': 'до передачи'});
    await bob.sync();
    await eve.join(node.gid, inviteCode: 'AAAA-BBBB-CCCC', displayName: 'Ева');
    await owner.sync();                     // Еве раздан ключ
    await eve.sync();
    await owner.transferOwner(node.gid, 'Гость-1');   // Бобу
    expect(owner.byId(node.gid)!.role, 'admin');
    await bob.sync();                       // Боб — новый владелец: создаёт поколение 2
    expect(bob.byId(node.gid)!.isOwner, isTrue);
    expect(node.epoch, 2);
    await bob.send(node.gid, GroupKind.text, {'t': 'я теперь владелец'});
    await owner.sync();                     // бывший владелец принимает ключ нового
    await eve.sync();                       // Ева закрепляла ключ прежнего владельца: по записи handover перезакрепляет нового
    expect(eve.hasProblem(node.gid), isFalse, reason: 'передача подтверждена изнутри группы');
    expect((await eve.messages(node.gid)).map((e) => e.body['t']), contains('я теперь владелец'));
    expect((await owner.messages(node.gid)).map((e) => e.body['t']), contains('я теперь владелец'));
  });

  test('передача владения: нельзя не владельцу; цепочку подтверждает только тот, чей ключ закреплён', () async {
    await owner.sync();
    await bobJoins();
    await expectLater(bob.transferOwner(node.gid, 'Хозяин-1'), throwsA(isA<GroupSendRefused>()));
    GroupEntry h(String from, String to, String pub) => GroupEntry(gid: node.gid, seq: 1, ts: 0, epoch: 1, kind: GroupKind.system, from: from, body: {'op': 'handover', 'to': to, 'pub': pub});
    final members = [
      {'alias': 'Старый', 'role': 'admin', 'member_pub': 'OLD'},
      {'alias': 'Новый', 'role': 'owner', 'member_pub': 'NEW'},
      {'alias': 'Рядовой', 'role': 'member', 'member_pub': 'MEM'},
    ];
    expect(GroupService.handoverNext([h('Старый', 'Новый', 'NEW')], members, 'OLD', {'NEW'}), 'NEW', reason: 'от закреплённого (прежнего) владельца');
    expect(GroupService.handoverNext([h('Рядовой', 'Новый', 'NEW')], members, 'OLD', {'NEW'}), isNull, reason: 'рядовой не подтверждает');
    expect(GroupService.handoverNext([h('Новый', 'Новый', 'NEW')], members, 'OLD', {'NEW'}), isNull, reason: 'сам новый себя не подтверждает');
    expect(GroupService.handoverNext([h('Старый', 'Новый', 'NEW')], members, 'OLD', {'OTHER'}), isNull, reason: 'ключ не тот, что нужен');
    expect(GroupService.handoverNext([], members, 'OLD', {'NEW'}), isNull);
  });
;

  test('забыть группу: удаляются ключи любых поколений (в т.ч. ≥ 64) и только этой группы', () {
    final g = 'ab' * 16, other = 'cd' * 16;
    for (final k in ['grp_key_0_$g', 'grp_key_64_$g', 'grp_key_1000_$g', 'grp_mine_$g', 'grp_owner_$g', 'grp_invspent_${g}_AAAA-BBBB-CCCC']) {
      expect(secureKeyOfGroup(k, g), isTrue, reason: k);
    }
    for (final k in ['grp_key_64_$other', 'grp_known', 'node_token_x', 'grp_dl_$other', 'pref_secure_keyboard']) {
      expect(secureKeyOfGroup(k, g), isFalse, reason: k);
    }
  });

  test('участник был не в сети две передачи владения подряд: догоняет ключи по цепочке и читает новое', () async {
    final carol = GroupService(node.as('carol'), MemoryGroupStore());
    await owner.sync();
    await bobJoins();
    await eve.join(node.gid, inviteCode: 'AAAA-BBBB-CCCC', displayName: 'Ева');
    await carol.join(node.gid, inviteCode: 'AAAA-BBBB-CCCC', displayName: 'Кэрол');
    await owner.sync();
    for (final x in [bob, eve, carol]) { await x.syncAndAnnounce(); }
    await owner.send(node.gid, GroupKind.text, {'t': 'первое'});
    // Кэрол уходит из сети; владение переходит дважды
    await owner.transferOwner(node.gid, 'Гость-1');          // → Боб
    await bob.sync();                                          // поколение 2
    await eve.sync();
    await bob.transferOwner(node.gid, eve.byId(node.gid)!.myAlias);   // → Ева
    await eve.sync();                                          // поколение 3
    await eve.send(node.gid, GroupKind.text, {'t': 'после двух передач'});
    expect(node.epoch, greaterThanOrEqualTo(3));
    await carol.sync();                                        // Кэрол возвращается
    expect(carol.hasProblem(node.gid), isFalse);
    expect((await carol.messages(node.gid)).map((e) => e.body['t']), containsAll(['первое', 'после двух передач']));
  });

  test('стирание модератора доходит до того, кто был не в сети (без намёка)', () async {
    await owner.sync();
    await bobJoins();
    final seq = await owner.send(node.gid, GroupKind.text, {'t': 'лишнее'});
    await bob.sync();
    expect((await bob.messages(node.gid)).length, 1);
    await owner.moderate(node.gid, 'delete_entry', seq: seq);   // Боб в это время не в сети, намёк не получил
    await bob.sync();
    expect(await bob.messages(node.gid), isEmpty);
  });

  test('повтор той же записи под новым номером не показывается второй раз', () async {
    await owner.sync();
    await bobJoins();
    await owner.send(node.gid, GroupKind.text, {'t': 'один раз'});
    final orig = node.log.last;
    node.log.add({...orig, 'seq': node.log.length + 1, 'ts': 9999});   // узел (или кто-то) переложил тот же шифртекст
    await bob.sync();
    expect((await bob.messages(node.gid)).where((e) => e.body['t'] == 'один раз').length, 1);
  });
}
