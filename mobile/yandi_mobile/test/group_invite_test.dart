import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/group_crypto.dart';
import 'package:yandi_mobile/models/group_invite.dart';
import 'package:yandi_mobile/screens/group_chat_screen.dart';

void main() {
  const gid = 'abcdef0123456789abcdef0123456789';
  test('приглашение: оба формата, регистр, мусор', () {
    final a = GroupInvite.tryParse('YANDI-GROUP-1:$gid:abcd-ef12-3456')!;
    expect(a.gid, gid);
    expect(a.code, 'ABCD-EF12-3456');
    expect(GroupInvite.tryParse('  $gid ABCD-EF12-3456 ')!.code, 'ABCD-EF12-3456');
    expect(GroupInvite.tryParse(a.encode())!.gid, gid);
    for (final bad in ['', 'привет', '$gid:ABCD', 'zz$gid:ABCD-EF12-3456', 'YANDI-GROUP-1:short:ABCD-EF12-3456']) {
      expect(GroupInvite.tryParse(bad), isNull, reason: bad);
    }
  });
  test('ссылка в тексте — отдельный вид', () {
    expect(kindOfText('привет'), GroupKind.text);
    expect(kindOfText('смотри https://example.org/x'), GroupKind.link);
    expect(kindOfText('WWW.example.org'), GroupKind.link);
  });
}
