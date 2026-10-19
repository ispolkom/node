import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/invite_payload.dart';
import 'package:yandi_mobile/screens/invite_device_screen.dart';

void main() {
  test('текст приглашения читается тем же разбором, что и код сопряжения: адрес, порт, отпечаток узла и 8-значный код', () {
    final t = inviteText(host: '203.0.113.7', port: 8444, code: '12345678', fingerprint: 'ab' * 32);
    expect(t.startsWith('YANDI-PAIR-1:'), isTrue);
    final j = jsonDecode(utf8.decode(base64.decode(t.substring('YANDI-PAIR-1:'.length)))) as Map<String, dynamic>;
    expect(j, {'host': '203.0.113.7', 'port': 8444, 'pairing_code': '12345678', 'tls_fingerprint': 'ab' * 32, 'tls': true});
  });

  testWidgets('экран: код и QR показываются, отсчёт идёт, по истечении и по «Отозвать» код исчезает; ошибка узла видна', (t) async {
    var revoked = 0;
    var makes = 0;
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async {
          makes++;
          if (makes == 1) throw StateError('дневной предел приглашений исчерпан');
          return (text: inviteText(host: 'h', port: 1, code: '87654321', fingerprint: 'cd' * 32), code: '87654321', ttlSecs: 3);
        },
        revoke: () async => revoked++,
      ),
    ));
    await t.tap(find.byKey(const ValueKey('invite-make')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(find.text('дневной предел приглашений исчерпан'), findsOneWidget, reason: 'текст ошибки узла показывается как есть, без деталей');
    expect(find.byKey(const ValueKey('invite-qr')), findsNothing);
    await t.tap(find.byKey(const ValueKey('invite-make')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(find.byKey(const ValueKey('invite-qr')), findsOneWidget);
    expect(find.text('Код: 87654321'), findsOneWidget);
    await t.pump(const Duration(seconds: 1));
    expect(find.textContaining('ещё 2 с'), findsOneWidget);
    // «Отозвать код» убирает код и сообщает узлу
    await t.scrollUntilVisible(find.byKey(const ValueKey('invite-revoke')), 200, scrollable: find.byType(Scrollable).first);
    await t.tap(find.byKey(const ValueKey('invite-revoke')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(revoked, 1);
    expect(find.byKey(const ValueKey('invite-qr')), findsNothing);
    // новый код, который истекает сам
    await t.tap(find.byKey(const ValueKey('invite-make')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(find.byKey(const ValueKey('invite-qr')), findsOneWidget);
    await t.pump(const Duration(seconds: 4));
    expect(find.byKey(const ValueKey('invite-qr')), findsNothing, reason: 'истёкший код с экрана убирается');
    expect(find.byKey(const ValueKey('invite-make')), findsOneWidget);
  });
}
