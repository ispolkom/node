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
    expect(j, {'host': '203.0.113.7', 'port': 8444, 'pairing_code': '12345678', 'tls_fingerprint': 'ab' * 32, 'tls': true, 'invite': true});
  });

  testWidgets('экран: код и QR показываются, отсчёт идёт, по истечении и по «Отозвать» код исчезает; ошибка узла видна', (t) async {
    var revoked = 0;
    var makes = 0;
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async {
          makes++;
          if (makes == 1) throw StateError('дневной предел приглашений исчерпан');
          final tx = inviteText(host: 'h', port: 1, code: '87654321', fingerprint: 'cd' * 32);
          return (text: tx, link: inviteLink(tx), code: '87654321', pin: '4821', ttlSecs: 3);
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
    expect(find.text('Код: 87654321', skipOffstage: false), findsOneWidget);
    expect(find.text('PIN: 4821'), findsOneWidget, reason: 'PIN виден пригласившему, чтобы назвать его другим каналом');
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

  test('ссылка-приглашение: туда и обратно; чужая и битая ссылка отвергаются; срок читается из текста', () {
    final t = inviteText(host: 'h', port: 1, code: '12345678', fingerprint: 'ab' * 32, expiresAtSecs: 1900000000);
    final link = inviteLink(t);
    expect(link.startsWith('yandi://invite?d='), isTrue);
    expect(inviteTextFromLink(link), t);
    expect(inviteExpiryFromText(t), 1900000000);
    expect(inviteTextFromLink('https://evil.example/invite?d=abc'), isNull);
    expect(inviteTextFromLink('yandi://other?d=abc'), isNull);
    expect(inviteTextFromLink('yandi://invite?d=!!!'), isNull);
    expect(inviteTextFromLink('yandi://invite?d=${base64Url.encode(utf8.encode('не наш формат'))}'), isNull);
    expect(inviteExpiryFromText(inviteText(host: 'h', port: 1, code: '1', fingerprint: 'x')), isNull);
  });

  testWidgets('экран: после закрытия приложения живое приглашение возвращается вместе с PIN', (t) async {
    final tx = inviteText(host: 'h', port: 1, code: '11112222', fingerprint: 'cd' * 32);
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async => throw StateError('не должно вызываться'),
        revoke: () async {},
        restore: () async => (text: tx, link: inviteLink(tx), code: '11112222', pin: '0042', ttlSecs: 1800),
      ),
    ));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(find.text('PIN: 0042'), findsOneWidget);
    expect(find.text('Код: 11112222', skipOffstage: false), findsOneWidget);
    expect(find.textContaining('30 мин'), findsOneWidget);
  });

  testWidgets('после «Скопировать текст» всплывает окно с PIN: его надо назвать другим каналом', (t) async {
    final tx = inviteText(host: 'h', port: 1, code: '11112222', fingerprint: 'cd' * 32);
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async => throw StateError('нет'),
        revoke: () async {},
        restore: () async => (text: tx, link: inviteLink(tx), code: '11112222', pin: '0042', ttlSecs: 1800),
      ),
    ));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    await t.scrollUntilVisible(find.byKey(const ValueKey('invite-copy')), 200, scrollable: find.byType(Scrollable).first);
    await t.tap(find.byKey(const ValueKey('invite-copy')), warnIfMissed: false);
    await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    await t.pump();
    await t.pump(const Duration(milliseconds: 300));
    expect(find.byKey(const ValueKey('invite-pin-dialog')), findsOneWidget);
    expect(find.text('0042'), findsOneWidget);
    expect(find.textContaining('Не отправляйте ссылку-приглашение и PIN в одно место'), findsOneWidget);
  });

  testWidgets('ошибка отзыва: код остаётся на экране и ошибка видна (узел его ещё принимает)', (t) async {
    final tx = inviteText(host: 'h', port: 1, code: '11112222', fingerprint: 'cd' * 32);
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async => throw StateError('нет'),
        revoke: () async => throw StateError('Не удалось отозвать коды (503)'),
        restore: () async => (text: tx, link: inviteLink(tx), code: '11112222', pin: '0042', ttlSecs: 1800),
      ),
    ));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    await t.scrollUntilVisible(find.byKey(const ValueKey('invite-revoke')), 200, scrollable: find.byType(Scrollable).first);
    await t.tap(find.byKey(const ValueKey('invite-revoke')));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    expect(find.text('Не удалось отозвать коды (503)', skipOffstage: false), findsOneWidget);
    expect(find.byKey(const ValueKey('invite-qr'), skipOffstage: false), findsOneWidget, reason: 'код не убран: он ещё действует');
  });

  testWidgets('окно PIN закрывается, когда приглашение истекло, и не падает при копировании', (t) async {
    final tx = inviteText(host: 'h', port: 1, code: '11112222', fingerprint: 'cd' * 32);
    await t.pumpWidget(MaterialApp(
      home: InviteDeviceScreen(
        create: () async => throw StateError('нет'),
        revoke: () async {},
        restore: () async => (text: tx, link: inviteLink(tx), code: '11112222', pin: '0042', ttlSecs: 2),
      ),
    ));
    await t.pump();
    await t.pump(const Duration(milliseconds: 10));
    await t.scrollUntilVisible(find.byKey(const ValueKey('invite-copy')), 200, scrollable: find.byType(Scrollable).first);
    await t.tap(find.byKey(const ValueKey('invite-copy')), warnIfMissed: false);
    await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    await t.pump();
    await t.pump(const Duration(milliseconds: 300));
    expect(find.byKey(const ValueKey('invite-pin-dialog')), findsOneWidget);
    await t.pump(const Duration(seconds: 3));
    await t.pump(const Duration(milliseconds: 300));
    expect(find.byKey(const ValueKey('invite-pin-dialog')), findsNothing, reason: 'мёртвый PIN не остаётся на экране');
    expect(t.takeException(), isNull);
  });
}
