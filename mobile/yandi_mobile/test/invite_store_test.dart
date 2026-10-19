import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/invite_store.dart';

class _Mem implements InviteBackend {
  final m = <String, String>{};
  @override
  Future<String?> read(String k) async => m[k];
  @override
  Future<void> write(String k, String v) async => m[k] = v;
  @override
  Future<void> delete(String k) async => m.remove(k);
}

void main() {
  late _Mem mem;
  var now = 1000000;
  setUp(() {
    mem = _Mem();
    now = 1000000;
    InviteStore.backend = mem;
    InviteStore.nowMs = () => now;
  });

  test('выданное приглашение живёт час и переживает «закрытие приложения»; после срока стирается', () async {
    await InviteStore.saveIssued(node: 'n1', text: 'YANDI-PAIR-1:x', code: '12345678', pin: '4821', expMs: now + 3600 * 1000);
    now += 59 * 60 * 1000;
    final m = await InviteStore.issued();
    expect(m?['pin'], '4821');
    now += 2 * 60 * 1000;
    expect(await InviteStore.issued(), isNull);
    expect(mem.m.containsKey('invite.issued'), isFalse, reason: 'просроченное стёрто, а не лежит');
  });

  test('принятая ссылка ждёт PIN час; после ввода (clear) исчезает; повреждённая запись игнорируется', () async {
    await InviteStore.savePending(text: 'YANDI-PAIR-1:y', expMs: now + 1000);
    expect((await InviteStore.pending())?['text'], 'YANDI-PAIR-1:y');
    await InviteStore.clearPending();
    expect(await InviteStore.pending(), isNull);
    mem.m['invite.pending'] = 'не json';
    expect(await InviteStore.pending(), isNull);
    expect(mem.m.containsKey('invite.pending'), isFalse);
  });
}
