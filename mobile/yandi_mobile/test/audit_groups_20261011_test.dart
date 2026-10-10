import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/group_crypto.dart';
import 'package:yandi_mobile/services/group_service.dart';
import 'package:yandi_mobile/services/group_store.dart';
import 'group_service_test.dart' as fixture;

void main() {
  test('bug_zero_member_key_blocks_owner_provisioning', () async {
    final node = fixture.FakeNode('owner');
    final owner = GroupService(node.as('owner'), MemoryGroupStore());
    await owner.sync();
    await node.as('attacker').join(node.gid, inviteCode: 'AAAA-BBBB-CCCC', memberPub: base64.encode(List.filled(32, 0)));
    await expectLater(owner.sync(), completes);
  });

  test('bug_offline_member_cannot_catch_up_across_handover', () async {
    final node = fixture.FakeNode('owner');
    final owner = GroupService(node.as('owner'), MemoryGroupStore());
    final laggingStore = MemoryGroupStore();
    final lagging = GroupService(node.as('lagging'), laggingStore);
    final next = GroupService(node.as('next'), MemoryGroupStore());
    await owner.sync();
    await node.as('lagging').join(node.gid, inviteCode: 'AAAA-BBBB-CCCC');
    await node.as('next').join(node.gid, inviteCode: 'AAAA-BBBB-CCCC');
    await lagging.sync();
    await next.sync();
    await owner.sync();
    await lagging.sync();
    await next.sync();
    expect(await laggingStore.read('grp_key_1_${node.gid}'), isNotNull);
    // Клиент lagging теперь offline: пропускает rekey и последующий handover.
    node.rekeyNeeded = true;
    await owner.sync();
    await owner.transferOwner(node.gid, node.members['next']!['alias'] as String);
    await next.sync();
    expect(node.epoch, greaterThanOrEqualTo(3));
    await lagging.sync();
    expect(lagging.hasProblem(node.gid), isFalse, reason: 'честная передача владельца должна поддерживать offline-клиентов');
    expect(await laggingStore.read('grp_key_2_${node.gid}'), isNotNull);
  });

  test('bug_offline_client_misses_moderator_deletion', () async {
    final node = fixture.FakeNode('owner');
    final owner = GroupService(node.as('owner'), MemoryGroupStore());
    final local = MemoryGroupStore();
    final bob = GroupService(node.as('bob'), local);
    await owner.sync();
    await node.as('bob').join(node.gid, inviteCode: 'AAAA-BBBB-CCCC');
    await bob.sync();
    await owner.sync();
    await bob.sync();
    final seq = await owner.send(node.gid, GroupKind.text, {'t': 'удаляемое'});
    await bob.sync();
    expect((await local.entries(node.gid)).any((e) => e.seq == seq && !e.deleted), isTrue);
    // Bob offline: hint потерян; после reconnect доступен обычный sync.
    await owner.moderate(node.gid, 'delete_entry', seq: seq);
    await bob.sync();
    expect((await local.entries(node.gid)).firstWhere((e) => e.seq == seq).deleted, isTrue);
  });
}
