import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/calls.dart';
import 'shell_m2_test.dart' as chat;
import 'widget_test.dart' as shell;

class ToolbarApi extends chat.FeatureApi {
  final invites = <Map<String, dynamic>>[];
  ToolbarApi() {
    rows.removeRange(1, rows.length);
    rows.single['short_id'] = chat.alice;
    rows.single['is_phone'] = true;
    final now = DateTime.now().millisecondsSinceEpoch;
    historyRows = [for (var i = 0; i < 50; i++) {
      'msg_id': 'message-$i', 'from': chat.alice,
      'timestamp': now - 60000 + i * 1000,
      'text': 'Сообщение $i\nСтрока истории\nЕщё строка истории\nКонец сообщения',
      'status': 'Delivered',
    }];
  }

  @override
  Future<Map<String, dynamic>> request(String path, {
    Map<String, dynamic>? body, String? method,
  }) async {
    if (path.endsWith('incoming-call')) return {'incoming': null};
    if (path.startsWith('/api/mobile/call/events')) return {'last': 0, 'events': []};
    if (path == '/api/mobile/call/signal') {
      final text = body!['text'] as String;
      final signal = jsonDecode(text.substring('\u0001yandi-call:'.length)) as Map<String, dynamic>;
      if (signal['t'] == 'invite') invites.add(signal);
      return {'status': 'ok'};
    }
    return super.request(path, body: body, method: method);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(PlatformFiles.channel, (_) async => null);
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(PlatformFiles.channel, null);
  });

  for (final width in [1280.0, 640.0]) {
    for (final video in [false, true]) {
      testWidgets('toolbar stays visible at width $width; ${video ? 'video' : 'audio'} call works after scrolling', (t) async {
        t.view.physicalSize = Size(width, 900);
        t.view.devicePixelRatio = 1;
        addTearDown(t.view.resetPhysicalSize);
        addTearDown(t.view.resetDevicePixelRatio);
        final api = ToolbarApi();
        await chat.open(t, api, 'Чат');
        await shell.click(t, 'Открыть чат');
        final toolbar = find.byKey(const ValueKey('chat-toolbar'));
        final history = find.byKey(const ValueKey('chat-history-scroll'));
        await t.ensureVisible(toolbar);
        await t.pumpAndSettle();
        final top = t.getTopLeft(toolbar);
        final scrollable = find.descendant(of: history, matching: find.byType(Scrollable));
        expect(t.state<ScrollableState>(scrollable).position.maxScrollExtent, greaterThan(1000));
        await t.drag(history, const Offset(0, -1800));
        await t.pumpAndSettle();
        expect(t.state<ScrollableState>(scrollable).position.pixels, greaterThan(1000));
        expect(t.getTopLeft(toolbar), top);
        for (final tip in ['Позвонить', 'Видеозвонок', 'Файлы чата', 'Очистить историю']) {
          expect(find.byTooltip(tip).hitTestable(), findsOneWidget);
        }

        await t.tap(find.byTooltip('Файлы чата'));
        await t.pumpAndSettle();
        expect(t.getTopLeft(toolbar), top);
        expect(find.byTooltip('Назад к сообщениям').hitTestable(), findsOneWidget);
        await t.tap(find.byTooltip('Назад к сообщениям'));
        await t.pumpAndSettle();
        expect(api.calls.where((p) => p.startsWith('/api/chat/clear/')), isEmpty);
        final controller = t.widget<CallsFeature>(find.byType(CallsFeature)).controller!;
        expect(controller.ready, isTrue,
            reason: 'The app-wide call controller must remain attached to the current call panel.');
        await t.tap(find.byTooltip(video ? 'Видеозвонок' : 'Позвонить'));
        await t.pumpAndSettle();
        expect(api.invites, hasLength(1));
        expect(api.invites.single['video'], video);
        expect(controller.running, isTrue);
        expect(api.calls.where((p) => p.startsWith('/api/chat/clear/')), isEmpty);
        await shell.stop(t);
        expect(controller.ready, isFalse);
        expect(controller.running, isFalse);
      });
    }
  }
}
