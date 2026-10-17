import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/advanced_settings.dart';
import 'package:yandi_desktop/theme.dart';
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
            'auth': 'password',
            'user': 'u',
            'has_password': true,
            'state': 'ok',
          },
        ],
      };
    if (path == '/api/vault/panic_erase') {
      calls.add(path);
      bodies.add(Map.of(body!));
      methods.add('POST');
      return {'ok': true};
    }
    if (path == '/api/mail-policy') {
      if (body != null) {
        calls.add(path);
        bodies.add(Map.of(body));
        methods.add('POST');
        return {'ok': true};
      }
      return {'days': 7, 'min': 1, 'max': 30, 'default': 7};
    }
    if (path == '/api/guest-pairing') {
      if (body != null) {
        calls.add(path);
        bodies.add(Map.of(body));
        methods.add('POST');
        return {'ok': true};
      }
      return {
        'enabled': false,
        'guests': 3,
        'max_guests': 64,
        'limits': {'max_file_bytes': 31457280, 'max_files_per_day': 20, 'max_bytes_per_day': 314572800},
      };
    }
    if (path.endsWith('/reveal')) {
      calls.add(path);
      return {'ok': true, 'user': 'u', 'password': 'SECRET'};
    }
    if (path == '/api/upstream-proxies/test') {
      calls.add(path);
      bodies.add(Map.of(body!));
      return {
        'ok': true,
        'results': [
          {'ok': true, 'ms': 42, 'proxy': 'example.com:1080'},
        ],
        'errors': [],
      };
    }
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
  testWidgets('no network section: ports are fixed and never edited or read', (
    t,
  ) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await panel(t, api, errors);
    expect(find.textContaining('порт'), findsNothing);
    expect(find.text('Сохранить порты и журнал'), findsNothing);
    expect(find.text('Адрес bind'), findsNothing);
    expect(api.calls.contains('/api/settings'), false);
    expect(errors, isEmpty);
    await m1.stop(t);
  });
  Finder inDialog(String label) => find.descendant(
    of: find.byType(AlertDialog),
    matching: find.widgetWithText(ElevatedButton, label),
  );
  testWidgets('proxy passwords never go to plain daemon storage', (t) async {
    final api = SettingsApi()
      ..configured = false
      ..unlocked = false;
    final errors = <Object>[];
    await panel(t, api, errors);
    await m1.click(t, 'Добавить');
    await t.enterText(
      find.widgetWithText(TextField, 'ip:port:login:pass'),
      'example.com:1080:u:SECRET',
    );
    await t.tap(inDialog('Добавить'));
    await t.pumpAndSettle();
    expect(errors.single, isA<ApiFailure>());
    expect(api.calls.contains('/api/upstream-proxies'), false);
    expect(find.text('SECRET'), findsNothing);
    await m1.stop(t);
  });
  testWidgets('proxy list: login and password hidden until the eye, ping, add, edit, delete', (t) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await panel(t, api, errors);
    // адрес виден, логин и пароль скрыты
    expect(find.textContaining('example.com:1080:••••••:••••••'), findsOneWidget);
    expect(find.textContaining('SECRET'), findsNothing);
    await t.tap(find.byIcon(Icons.visibility));
    await t.pumpAndSettle();
    expect(find.textContaining('example.com:1080:u:SECRET'), findsOneWidget);
    await t.tap(find.byIcon(Icons.visibility_off));
    await t.pumpAndSettle();
    expect(find.textContaining('SECRET'), findsNothing);
    // «Изменить» и «Удалить» — только при выбранных
    ElevatedButton btn(String l) =>
        t.widget<ElevatedButton>(find.widgetWithText(ElevatedButton, l));
    expect(btn('Изменить').onPressed, isNull);
    expect(btn('Удалить').onPressed, isNull);
    // добавление: пинг до сохранения
    await m1.click(t, 'Добавить');
    await t.enterText(
      find.widgetWithText(TextField, 'ip:port:login:pass'),
      'example.com:1080:u:SECRET',
    );
    await t.tap(inDialog('Пинг'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'text': 'example.com:1080:u:SECRET', 'kind': 'socks5'});
    expect(find.textContaining('работает, 42 мс'), findsOneWidget);
    expect(api.methods.contains('POST') && api.calls.contains('/api/upstream-proxies'), false);
    await t.tap(inDialog('Добавить'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'text': 'example.com:1080:u:SECRET', 'kind': 'socks5'});
    // кривая строка не уходит на узел
    final before = api.bodies.length;
    await m1.click(t, 'Добавить');
    await t.enterText(find.widgetWithText(TextField, 'ip:port:login:pass'), 'example.com');
    await t.tap(inDialog('Добавить'));
    await t.pumpAndSettle();
    expect(find.textContaining('Формат: ip:port:login:pass'), findsOneWidget);
    expect(api.bodies.length, before);
    await t.tap(find.text('Отмена'));
    await t.pumpAndSettle();
    // выбрать, изменить
    await t.tap(find.byType(PanelCheck).at(1));
    await t.pumpAndSettle();
    expect(btn('Изменить').onPressed, isNotNull);
    await m1.click(t, 'Изменить');
    expect(
      t.widget<TextField>(find.widgetWithText(TextField, 'ip:port:login:pass')).controller!.text,
      'example.com:1080:u:SECRET',
    );
    await t.enterText(
      find.widgetWithText(TextField, 'Клиентов (0 — общий предел)'),
      '3',
    );
    await t.tap(inDialog('Сохранить'));
    await t.pumpAndSettle();
    expect(api.bodies.last['max_clients'], 3);
    expect(api.bodies.last['host'], 'example.com');
    expect(api.bodies.last['password'], 'SECRET');
    // удалить с подтверждением
    await m1.click(t, 'Удалить');
    await m1.click(t, 'Отмена');
    expect(api.methods.contains('DELETE'), false);
    await m1.click(t, 'Удалить');
    await t.tap(find.descendant(of: find.byType(AlertDialog), matching: find.text('Подтвердить')));
    await t.pumpAndSettle();
    expect(api.methods.contains('DELETE'), true);
    // настройки пула сохраняются одной кнопкой
    await t.tap(find.text('Включить пул'));
    await t.pumpAndSettle();
    await m1.click(t, 'Сохранить настройки прокси');
    expect(api.bodies.last, {'enabled': true, 'strict': true, 'max_clients': 4});
    expect(errors, isEmpty);
    await m1.stop(t);
  });
  testWidgets('pairing without QR: off by default, the owner turns it on', (t) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await t.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: GuestPairingPanel(api: api, busy: false, isActive: () => true, mutate: (a) async {
              try { await a(); } catch (e) { errors.add(e); }
            }),
          ),
        ),
      ),
    );
    await t.pumpAndSettle();
    expect(find.text('Гостей сейчас: 3 из 64'), findsOneWidget);
    expect(find.textContaining('не больше 30 МБ'), findsOneWidget);
    expect(find.textContaining('20 файлов и 300 МБ'), findsOneWidget);
    await t.tap(find.text('Разрешить сопряжение без QR'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'enabled': true});
    expect(errors, isEmpty);
    await m1.stop(t);
  });
  test('proxy line parser understands every form the node does, and round-trips IPv6', () {
    ProxyLine? p(String s) => ProxyLine.parse(s);
    expect(p('1.2.3.4:1080')!.host, '1.2.3.4');
    expect(p('1.2.3.4:1080:u:pa:ss')!.password, 'pa:ss', reason: 'двоеточие внутри пароля');
    expect(p('u:pw@1.2.3.4:1080')!.user, 'u');
    expect(p('socks5://u:pw@[2001:db8::1]:1080')!.host, '2001:db8::1');
    expect(p('http://example.com:3128/')!.port, 3128);
    expect(p('[2001:db8::1]:1080')!.host, '2001:db8::1');
    expect(p('[2001:db8::1]:1080:u:pw')!.user, 'u');
    for (final bad in ['example.com', '1.2.3.4:0', '1.2.3.4:70000', ':1080', '[::1]', '[::1]:x', 'a:b@host', '1.2.3.4:1080:u']) {
      expect(p(bad), isNull, reason: bad);
    }
    // форма правки: сериализация и разбор обратно дают то же самое, в том числе для IPv6
    final text = ProxyLine.format('2001:db8::1', 1080, user: 'u', password: 'p:w');
    final back = ProxyLine.parse(text)!;
    expect((back.host, back.port, back.user, back.password), ('2001:db8::1', 1080, 'u', 'p:w'));
    expect(ProxyLine.parse(ProxyLine.format('h.example', 80))!.host, 'h.example');
  });
  testWidgets('mail retention: shows the term, refuses out-of-range, saves a valid one', (t) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await t.pumpWidget(MaterialApp(home: Scaffold(body: SingleChildScrollView(child: MailPolicyPanel(api: api, busy: false, isActive: () => true, mutate: (a) async { try { await a(); } catch (e) { errors.add(e); } })))));
    await t.pumpAndSettle();
    expect(t.widget<TextField>(find.byType(TextField)).controller!.text, '7');
    await t.enterText(find.byType(TextField), '99');
    await m1.click(t, 'Сохранить срок');
    expect(find.textContaining('от 1 до 30'), findsOneWidget);
    expect(api.bodies.where((b) => b.containsKey('days')), isEmpty, reason: 'вне пределов на узел не уходит');
    await t.enterText(find.byType(TextField), '3');
    await m1.click(t, 'Сохранить срок');
    expect(api.bodies.last, {'days': 3});
    expect(find.text('Срок хранения почты сохранён.'), findsOneWidget);
    expect(errors, isEmpty);
    await m1.stop(t);
  });
  testWidgets('emergency erase: needs the confirmation word, the wrong word sends nothing', (t) async {
    final api = SettingsApi();
    final errors = <Object>[];
    await t.pumpWidget(MaterialApp(home: Scaffold(body: SingleChildScrollView(child: PanicPanel(api: api, busy: false, isActive: () => true, mutate: (a) async { try { await a(); } catch (e) { errors.add(e); } })))));
    await t.pumpAndSettle();
    await m1.click(t, 'Стереть закрытую среду');
    await t.enterText(find.byType(TextField), 'нет');
    await t.tap(inDialog('Стереть'));
    await t.pumpAndSettle();
    expect(find.text('Слово не совпало: ничего не стёрто.'), findsOneWidget);
    expect(api.calls.contains('/api/vault/panic_erase'), false, reason: 'с неверным словом на узел ничего не уходит');
    await m1.click(t, 'Стереть закрытую среду');
    await t.enterText(find.byType(TextField), 'СТЕРЕТЬ');
    await t.tap(inDialog('Стереть'));
    await t.pumpAndSettle();
    expect(api.bodies.last, {'confirm': 'СТЕРЕТЬ'});
    expect(find.text('Закрытая среда стёрта.'), findsOneWidget);
    expect(errors, isEmpty);
    await m1.stop(t);
  });
  testWidgets('About screen', (t) async {
    await m2.open(t, SettingsApi(), 'О приложении');
    expect(find.text('YANDI Desktop · 0.1.0'), findsOneWidget);
    await m1.stop(t);
  });
}
