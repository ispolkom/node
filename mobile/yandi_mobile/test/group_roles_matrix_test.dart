import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/group.dart';

GroupInfo as(String role) => GroupInfo.fromJson({'id': 'g' * 32, 'name': 'Г', 'role': role, 'my_alias': 'me'});

void main() {
  group('кто может стереть запись автора (как на узле)', () {
    test('рядовой участник — никого, даже себя: он только скрывает автора у себя', () {
      final g = as('member');
      expect(g.canDeleteEntryOf('member', mine: false), false);
      expect(g.canDeleteEntryOf('member', mine: true), false);
    });
    test('модератор — рядовых и свои, не модераторов, админов и владельца', () {
      final g = as('moderator');
      expect(g.canDeleteEntryOf('member', mine: false), true);
      expect(g.canDeleteEntryOf(null, mine: false), true, reason: 'автор уже ушёл из группы');
      expect(g.canDeleteEntryOf('moderator', mine: false), false);
      expect(g.canDeleteEntryOf('admin', mine: false), false);
      expect(g.canDeleteEntryOf('owner', mine: false), false);
      expect(g.canDeleteEntryOf('moderator', mine: true), true);
    });
    test('админ — рядовых и модераторов, не админов и владельца', () {
      final g = as('admin');
      expect(g.canDeleteEntryOf('member', mine: false), true);
      expect(g.canDeleteEntryOf('moderator', mine: false), true);
      expect(g.canDeleteEntryOf('admin', mine: false), false);
      expect(g.canDeleteEntryOf('owner', mine: false), false);
    });
    test('владелец — всех, кроме чужого владельца (он один)', () {
      final g = as('owner');
      for (final r in ['member', 'moderator', 'admin']) {
        expect(g.canDeleteEntryOf(r, mine: false), true);
      }
      expect(g.canDeleteEntryOf('owner', mine: true), true);
    });
  });

  group('к кому можно применить заглушить / выгнать / запретить вход', () {
    test('модератор — только рядовых', () {
      final g = as('moderator');
      expect(g.canActOnRole('member'), true);
      expect(g.canActOnRole('moderator'), false);
      expect(g.canActOnRole('admin'), false);
      expect(g.canActOnRole('owner'), false);
    });
    test('админ — рядовых и модераторов', () {
      final g = as('admin');
      expect(g.canActOnRole('member'), true);
      expect(g.canActOnRole('moderator'), true);
      expect(g.canActOnRole('admin'), false);
    });
    test('владелец — всех, кроме себя', () {
      final g = as('owner');
      expect(g.canActOnRole('admin'), true);
      expect(g.canActOnRole('owner'), false);
    });
    test('рядовой — никого', () {
      expect(as('member').canActOnRole('member'), false);
    });
  });

  test('права управления: владелец и админ управляют, модератор только модерирует', () {
    expect(as('owner').canManage, true);
    expect(as('admin').canManage, true);
    expect(as('moderator').canManage, false);
    expect(as('moderator').canModerate, true);
    expect(as('member').canModerate, false);
  });
}
