import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'widget_test.dart' show FakeApi, start, click, stop;

class NetworkApi extends FakeApi {
  final gateways = <Map<String, dynamic>>[];
  bool relay = false,
      proxy = false,
      socks = false,
      stopFails = false,
      saveFails = false;
  Completer<Map<String, dynamic>>? late;
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/gateways') {
      calls.add(path);
      if (body != null && !saveFails) {
        if (method == 'DELETE') {
          gateways.removeWhere((r) => r['id'] == body['id']);
        } else {
          gateways.removeWhere((r) => r['id'] == body['id']);
          gateways.add({...body, 'id': body['id'] ?? 'g1', 'connected': true});
        }
      }
      return {'gateways': List.of(gateways)};
    }
    if (path == '/api/proxy/status')
      return {
        'active': proxy,
        'proxies': proxy
            ? [
                {'short_id': '0123456789abcdef', 'local_port': 9876},
              ]
            : [],
      };
    if (path == '/api/socks5/status')
      return {
        'active': socks,
        'proxies': socks
            ? [
                {'short_id': '0123456789abcdef', 'local_port': 12345},
              ]
            : [],
      };
    if (path.contains('/socks5/start/')) {
      calls.add(path);
      socks = true;
      return {
        'listen_addr': '127.0.0.1:12345',
        'username': 'yandi',
        'password': 'ephemeral-fixture-password',
      };
    }
    if (path.contains('/socks5/stop/')) {
      calls.add(path);
      socks = false;
      return {'ok': true};
    }
    if (path.contains('/proxy/start/')) {
      calls.add(path);
      proxy = true;
      return {'local_port': 9876};
    }
    if (path.contains('/proxy/stop/')) {
      calls.add(path);
      if (stopFails) throw const ApiFailure('http', 501);
      proxy = false;
      return {'ok': true};
    }
    if (path == '/api/network/relay') {
      if (late != null) return late!.future;
      if (body != null) {
        calls.add(path);
        relay = body['enabled'] == true;
        return {'status': 'success', 'relay_enabled': relay};
      }
      return {
        'client': false,
        'relay_enabled': relay,
        'records': 0,
        'forwarding': {'active_sessions': 1, 'bytes': 123, 'packets': 2},
      };
    }
    if (path == '/api/nodes')
      return {
        'nodes': [
          {'short_id': '0123456789abcdef', 'source': 'p2p', 'hostname': 'peer'},
          {'short_id': 'mdns', 'source': 'mdns'},
        ],
      };
    return super.request(path, body: body, method: method);
  }
}

void main() {
  testWidgets('gateway CRUD, HTTP errors and confirmed delete', (t) async {
    final api = NetworkApi();
    await start(t, api);
    await click(t, 'Войти');
    await click(t, 'Шлюзы');
    await t.enterText(find.byType(TextField).first, '<b>шлюз</b>');
    await t.enterText(find.byType(TextField).last, '0123456789abcdef');
    await click(t, 'Добавить шлюз');
    expect(find.textContaining('<b>шлюз</b>'), findsWidgets);
    await click(t, 'Изменить');
    await t.enterText(find.byType(TextField).first, 'Новое имя');
    await click(t, 'Сохранить шлюз');
    expect(api.gateways.single['name'], 'Новое имя');
    await click(t, 'Старт HTTP');
    expect(find.textContaining('9876'), findsWidgets);
    api.stopFails = true;
    await click(t, 'Стоп HTTP');
    expect(api.proxy, true);
    expect(
      find.textContaining('Действие не реализовано демоном'),
      findsOneWidget,
    );
    api.stopFails = false;
    await click(t, 'Стоп HTTP');
    expect(
      find.textContaining('Закрытие слушающего сокета API не подтверждает'),
      findsOneWidget,
    );
    await click(t, 'Удалить');
    expect(find.byType(AlertDialog), findsOneWidget);
    await click(t, 'Отмена');
    expect(api.gateways, isNotEmpty);
    await click(t, 'Удалить');
    await t.tap(find.widgetWithText(FilledButton, 'Удалить'));
    await t.pumpAndSettle();
    expect(api.gateways, isEmpty);
    await stop(t);
  });
  testWidgets('SOCKS credential stays ephemeral and navigation clears it', (
    t,
  ) async {
    final api = NetworkApi();
    api.gateways.add({
      'id': 'g1',
      'name': 'gateway',
      'short_id': '0123456789abcdef',
      'country': 'RU',
      'connected': true,
    });
    await start(t, api);
    await click(t, 'Войти');
    await click(t, 'Шлюзы');
    await click(t, 'Старт SOCKS5');
    expect(find.textContaining('ephemeral-fixture-password'), findsOneWidget);
    final startButton = t.widget<OutlinedButton>(
      find.widgetWithText(OutlinedButton, 'Старт HTTP'),
    );
    expect(startButton.onPressed, isNull);
    await click(t, 'Стоп SOCKS5');
    expect(api.socks, false);
    expect(find.textContaining('ephemeral-fixture-password'), findsNothing);
    await click(t, 'Старт SOCKS5');
    await click(t, 'Главная');
    await click(t, 'Шлюзы');
    expect(find.textContaining('ephemeral-fixture-password'), findsNothing);
    await stop(t);
  });
  testWidgets('ретрансляция включена по умолчанию, отдельного блока на Главной нет', (t) async {
    final api = NetworkApi()..relay = true;
    await start(t, api);
    await click(t, 'Войти');
    await click(t, 'Статус узла');
    expect(find.text('Ретрансляция'), findsNothing);
    expect(find.byKey(const ValueKey('relay-toggle')), findsNothing);
    expect(api.calls.contains('/api/network/relay'), false);
    expect(find.text('Relay'), findsNothing);
    await stop(t);
  });
  testWidgets('persistence error and preflight lock prevent success', (
    t,
  ) async {
    final api = NetworkApi()..saveFails = true;
    await start(t, api);
    await click(t, 'Войти');
    await click(t, 'Шлюзы');
    await t.enterText(find.byType(TextField).first, 'name');
    await t.enterText(find.byType(TextField).last, '0123456789abcdef');
    await click(t, 'Добавить шлюз');
    expect(find.textContaining('отклонил действие'), findsOneWidget);
    api.configured = true;
    api.unlocked = false;
    final count = api.calls.where((p) => p == '/api/gateways').length;
    await click(t, 'Добавить шлюз');
    expect(api.calls.where((p) => p == '/api/gateways').length, count);
    await stop(t);
  });
  testWidgets('network pages usable in small window', (t) async {
    await t.binding.setSurfaceSize(const Size(400, 500));
    addTearDown(() => t.binding.setSurfaceSize(null));
    await start(t, NetworkApi());
    await click(t, 'Войти');
    await click(t, 'Шлюзы');
    expect(t.takeException(), isNull);
    await stop(t);
  });
}
