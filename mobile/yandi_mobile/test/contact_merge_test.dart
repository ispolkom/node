import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact.dart';
import 'package:yandi_mobile/models/contact_merge.dart';

String id(String c) => c.padRight(64, '0');
Map<String, dynamic> u(String c, String name, String role, {bool online = false, String nick = ''}) =>
    {'peer_id': id(c), 'name': name, 'nick': nick, 'role': role, 'online': online, 'last_activity_ms': 0};

void main() {
  bool none(String _) => false;

  test('сохранённый гость остаётся гостем и «в сети»: роль и присутствие от узла, имя своё', () {
    final r = mergeContacts(
      fromNode: const [],
      catalog: [u('a', 'Телефон 1', 'guest', online: true)],
      savedNames: {id('a'): 'Антон'},
      isBlocked: none,
    );
    expect(r.length, 1);
    expect(r.single.displayName, 'Антон');
    expect(r.single.role, 'guest');
    expect(r.single.online, isTrue);
    expect(r.single.saved, isTrue);
  });

  test('владелец: контакты узла с ролью; переименование не теряет роль и присутствие', () {
    final r = mergeContacts(
      fromNode: [Contact(peerId: id('b'), displayName: '📱 Гость', online: true, isManual: false, role: 'guest')],
      catalog: const [],
      savedNames: {id('b'): 'Борис'},
      isBlocked: none,
    );
    expect(r.single.role, 'guest');
    expect(r.single.online, isTrue);
    expect(r.single.displayName, 'Борис');
  });

  test('каталог без контактов: все в сети и не в сети, ник сильнее имени устройства; скрытый ник — имя устройства', () {
    final r = mergeContacts(
      fromNode: const [],
      catalog: [u('c', 'Телефон c', 'user', online: true, nick: 'Мама'), u('d', 'Телефон d', 'user'), u('e', 'Гость e', 'guest')],
      savedNames: const {},
      isBlocked: none,
    );
    expect(r.map((c) => c.displayName), ['Мама', 'Телефон d', 'Гость e']);
    expect(r.map((c) => c.role), ['user', 'user', 'guest']);
    expect(r.map((c) => c.online), [true, false, false]);
  });

  test('компьютер узла доступен для общения; заблокированные локально не попадают в список', () {
    final r = mergeContacts(
      fromNode: const [],
      catalog: [{'peer_id': id('f'), 'name': 'Компьютер', 'is_node': true, 'online': true}, u('g', 'Телефон g', 'user')],
      savedNames: const {},
      isBlocked: (p) => p == id('g'),
    );
    expect(r.single.peerId, id('f'));
    expect(r.single.displayName, '💻 Компьютер');
  });

  test('знакомый только по номеру остаётся в списке (не в сети), если узел его не знает', () {
    final r = mergeContacts(fromNode: const [], catalog: const [], savedNames: {id('h'): 'Дальний'}, isBlocked: none);
    expect(r.single.displayName, 'Дальний');
    expect(r.single.online, isFalse);
    expect(r.single.isManual, isTrue);
  });

  test('приглашённый по цепочке доверия — пользователь, не гость', () {
    expect(Contact(peerId: 'p', displayName: 'x', online: false, isManual: false, role: 'invited').isGuestLevel, isFalse);
    expect(Contact(peerId: 'p', displayName: 'x', online: false, isManual: false, role: 'guest').isGuestLevel, isTrue);
  });
}
