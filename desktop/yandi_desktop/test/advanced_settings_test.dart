import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/advanced_settings.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

class SettingsApi extends m2.FeatureApi {
  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path == '/api/settings') {
      if (body != null) {
        calls.add(path);
        bodies.add(body);
        methods.add(method!);
        return {'requires_restart': true};
      }
      return {
        'network': {
          for (final key in [
            'discovery_port',
            'data_port',
            'mobile_gateway',
            'mobile_p2p',
            'http_proxy',
            'web_ui',
          ])
            key: 8443,
        },
        'server': {'bind_address': '127.0.0.1', 'log_level': 'info'},
      };
    }
    if (path == '/api/exit/settings') return {'mode': 'trusted'};
    if (path == '/api/upstream-proxies' && body == null)
      return {
        'max_clients': 4,
        'enabled': false,
        'strict': true,
        'proxies': [
          {
            'id': 'proxy1',
            'host': 'example.com',
            'port': 1080,
            'kind': 'socks5',
            'enabled': true,
            'cap': 4,
            'clients': 0,
          },
        ],
      };
    if (path.startsWith('/api/upstream-proxies')) {
      calls.add(path);
      if (body != null) bodies.add(Map.of(body));
      methods.add(method ?? 'POST');
      return {'ok': true};
    }
    return super.request(path, body: body, method: method);
  }
}

Future<void> panel(WidgetTester t, SettingsApi api, List<Object> errors) async {
  await t.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: AdvancedSettings(
            api: api,
            busy: false,
            isActive: () => true,
            mutate: (action) async {
              try {
                await action();
              } catch (e) {
                errors.add(e);
              }
            },
          ),
        ),
      ),
    ),
  );
  await t.pumpAndSettle();
}

void main() {
  testWidgets('ports validate and save PUT with restart banner', (t) async {
    final api = SettingsApi();
    await panel(t, api, []);
    await t.enterText(find.widgetWithText(TextField, 'Discovery порт'), '0');
    await m1.click(t, 'Сохранить порты и журнал');
    expect(api.methods.contains('PUT'), false);
    await t.ensureVisible(find.widgetWithText(TextField, 'Discovery порт'));
    await t.enterText(
      find.widgetWithText(TextField, 'Discovery порт'),
      '12345',
    );
    await m1.click(t, 'Сохранить порты и журнал');
    expect(api.bodies.last['network']['discovery_port'], 12345);
    expect(
      find.text('Нужен перезапуск узла для применения настроек.'),
      findsOneWidget,
    );
    await m1.stop(t);
  });
  testWidgets('proxy passwords never go to plain daemon storage', (t) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await panel(t, api, errors);
    await m1.click(t, 'Добавить внешний прокси');
    for (final e in {
      'Хост прокси': 'example.com',
      'Порт прокси': '1080',
      'Логин прокси (необязательно)': 'u',
      'Пароль прокси': 'SECRET',
    }.entries) {
      await t.enterText(find.widgetWithText(TextField, e.key), e.value);
    }
    await m1.click(t, 'Сохранить');
    expect(errors.single, isA<ApiFailure>());
    expect(api.calls.contains('/api/upstream-proxies'), false);
    expect(find.text('SECRET'), findsNothing);
    api.configured = api.unlocked = true;
    await m1.click(t, 'Изменить лимит прокси');
    await t.enterText(
      find.widgetWithText(TextField, 'Клиентов (0 — общий предел)'),
      '3',
    );
    await m1.click(t, 'Подтвердить');
    expect(api.bodies.last, {'max_clients': 3});
    await m1.click(t, 'Удалить внешний прокси');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.stop(t);
  });
  testWidgets('About screen', (t) async {
    await m2.open(t, SettingsApi(), 'О приложении');
    expect(find.text('YANDI Desktop · 0.1.0'), findsOneWidget);
    await m1.stop(t);
  });
}
