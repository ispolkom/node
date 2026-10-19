import 'dart:async';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/devices.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

class DeviceApi extends m2.FeatureApi {
  Completer<Map<String, dynamic>>? pairing;
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/mobile/pairing')
      return pairing?.future ??
          {
            'qr_text': 'daemon-secret',
            'expires_in_secs': 3,
            'qr_svg':
                '<svg viewBox="0 0 21 21"><path fill="#000" d="M4 4h1v1H4V4"/></svg>',
          };
    if (path == '/api/mobile/devices' || path == '/api/mobile/other-devices')
      return {
        'devices': [
          {
            'name': 'Phone',
            'peer_id': 'd' * 64,
            'online': true,
            'status': 'hidden',
            'connections': 1,
            'last_seen_ms': 0,
          },
        ],
      };
    if (path.startsWith('/api/mobile/')) {
      calls.add(path);
      methods.add(method ?? 'POST');
      if (body != null) bodies.add(body);
      return {'ok': true};
    }
    return super.request(path, body: body, method: method);
  }
}

class InviteDeviceApi extends DeviceApi {
  @override
  Future<Map<String, dynamic>> request(String path, {Map<String, dynamic>? body, String? method}) async {
    if (path == '/api/mobile/devices') {
      return {
        'devices': [
          {'name': 'Мама', 'peer_id': 'a' * 64, 'online': true, 'connections': 1, 'last_seen_ms': 0, 'role': 'user'},
          {'name': 'Тётя', 'peer_id': 'b' * 64, 'online': false, 'connections': 0, 'last_seen_ms': 0, 'role': 'invited', 'invited_by': 'a' * 64, 'invited_by_name': 'Мама'},
          {'name': 'Чужой', 'peer_id': 'c' * 64, 'online': false, 'connections': 0, 'last_seen_ms': 0, 'role': 'вредное <b>значение</b>'},
        ],
      };
    }
    return super.request(path, body: body, method: method);
  }
}

void main() {
  testWidgets('pairing TTL, confirmation and full ID revoke', (t) async {
    final api = DeviceApi();
    await m2.openAdmin(t, api, 'Настройки узла');
    await m1.click(t, 'Создать код сопряжения');
    expect(find.text('daemon-secret'), findsOneWidget);
    await t.pump(const Duration(seconds: 4));
    expect(find.text('daemon-secret'), findsNothing);
    await m1.click(t, 'Отозвать устройство');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.click(t, 'Отозвать устройство');
    await m1.click(t, 'Подтвердить');
    expect(api.calls.contains('/api/mobile/devices/${'d' * 64}'), true);
    await m1.stop(t);
  });
  testWidgets('pairing QR stays a 280x280 square', (t) async {
    final api = DeviceApi();
    await m2.openAdmin(t, api, 'Настройки узла');
    await t.ensureVisible(find.text('Создать код сопряжения'));
    await m1.click(t, 'Создать код сопряжения');
    final size = t.getSize(find.byKey(const ValueKey('pairing-qr')));
    expect(size, const Size(280, 280));
    await m1.stop(t);
  });
  testWidgets('late pairing is discarded on navigation', (t) async {
    final api = DeviceApi()..pairing = Completer();
    await m2.openAdmin(t, api, 'Настройки узла');
    await t.ensureVisible(find.text('Создать код сопряжения'));
    await t.tap(find.text('Создать код сопряжения'));
    await t.pump();
    await m1.stop(t);
    api.pairing!.complete({'qr_text': 'late-secret', 'expires_in_secs': 300});
    await t.pump();
    expect(find.text('late-secret'), findsNothing);
  });
  testWidgets('other devices actions require confirmation', (t) async {
    final api = DeviceApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Добавить устройство в контакты');
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.last, {'peer': 'd' * 64, 'action': 'add'});
    await m1.stop(t);
  });
  test('QR accepts only rectangles and rejects bounds', () {
    expect(
      QrSquares.parse(
        '<svg viewBox="0 0 21 21"><path fill="#000" d="M4 4h1v1H4V4"/></svg>',
      ).squares.length,
      1,
    );
    expect(
      () => QrSquares.parse(
        '<svg viewBox="0 0 21 21"><path fill="#000" d="M4 4h99v1H4V4"/></svg>',
      ),
      throwsA(anything),
    );
  });

  testWidgets('роль и пригласивший видны; повысить можно только приглашённого; отзыв ветки — с подтверждением', (t) async {
    final api = InviteDeviceApi();
    await m2.openAdmin(t, api, 'Настройки узла');
    expect(find.textContaining('Роль: Приглашённый · пригласил: Мама'), findsOneWidget);
    expect(find.textContaining('Роль: Пользователь'), findsOneWidget);
    expect(find.textContaining('Неизвестная роль'), findsOneWidget, reason: 'неизвестная роль от узла не показывается как есть');
    expect(find.textContaining('<b>'), findsNothing);
    expect(find.text('Повысить до пользователя'), findsOneWidget, reason: 'только у приглашённого');
    // повышение
    await t.ensureVisible(find.text('Повысить до пользователя'));
    await m1.click(t, 'Повысить до пользователя');
    await m1.click(t, 'Отмена');
    expect(api.calls.any((c) => c.endsWith('/promote')), false);
    await m1.click(t, 'Повысить до пользователя');
    await m1.click(t, 'Подтвердить');
    expect(api.calls.contains('/api/mobile/devices/${'b' * 64}/promote'), true);
    // отзыв ветки у того, кто пригласил (у «Мамы» есть приглашённый)
    final branch = find.byKey(ValueKey('branch-${'a' * 64}'));
    await t.ensureVisible(branch);
    await t.tap(branch);
    await t.pumpAndSettle();
    await m1.click(t, 'Подтвердить');
    expect(api.calls.contains('/api/mobile/devices/${'a' * 64}/revoke-branch'), true);
    await m1.stop(t);
  });
}
