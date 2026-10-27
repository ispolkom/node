import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/services/contact_cache.dart';

void main() {
  final id = 'a' * 64;
  late Map<String, String> disk;
  ContactCache store() => ContactCache(
    read: (key) async => disk[key],
    write: (key, value) async { disk[key] = value; },
  );
  setUp(() { disk = {}; });

  test('offline restart restores saved contacts, names and roles without stale presence', () async {
    await store().save('node-a', [Contact(peerId: id, displayName: 'Home',
      online: true, isManual: false, saved: true, role: 'user')]);
    final restored = await store().load('node-a', {}, (_) => false);
    expect(restored.single.displayName, 'Home');
    expect(restored.single.saved, isTrue);
    expect(restored.single.online, isFalse);
    expect(restored.single.role, 'user');
    expect(await store().load('node-b', {}, (_) => false), isEmpty);
  });

  test('first offline startup loads manual contacts even without a network snapshot', () async {
    final restored = await store().load('node-a', {id: 'Local friend'}, (_) => false);
    expect(restored.single.displayName, 'Local friend');
    expect(restored.single.saved, isTrue);
    expect(restored.single.online, isFalse);
  });

  test('local rename, empty saved name and blacklist survive cache restore', () async {
    await store().save('node-a', [Contact(peerId: id, displayName: 'Remote name',
      online: true, isManual: false)]);
    expect((await store().load('node-a', {id: 'My name'}, (_) => false)).single.displayName, 'My name');
    final unnamed = (await store().load('node-a', {id: ''}, (_) => false)).single;
    expect(unnamed.saved, isTrue);
    expect(unnamed.displayName, 'Remote name');
    expect(await store().load('node-a', {id: ''}, (_) => true), isEmpty);
  });

  test('corrupt snapshot does not hide independently saved contacts', () async {
    disk['contacts_snapshot_node-a'] = '{broken';
    expect((await store().load('node-a', {id: 'Friend'}, (_) => false)).single.displayName, 'Friend');
  });

  test('successful empty refresh replaces old snapshot', () async {
    await store().save('node-a', [Contact(peerId: id, displayName: 'Old', online: false, isManual: false)]);
    await store().save('node-a', []);
    expect(await store().load('node-a', {}, (_) => false), isEmpty);
  });
}
