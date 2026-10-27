import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/calls.dart';
import 'package:yandi_desktop/domain/call_signaling.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/preferences.dart';
import 'calls_test.dart' as fixture;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final calls = <MethodCall>[];
  setUp(() {
    calls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(PlatformFiles.channel, (call) async {
      calls.add(call);
      return null;
    });
  });

  test('private and group notifications select different bundled sounds; master mute silences both', () async {
    final prefs = AppPreferences();
    expect(prefs.sound, isTrue);
    await prefs.alert();
    expect((calls.last.arguments as Map)['group'], isNull);
    await prefs.alert(group: true);
    expect((calls.last.arguments as Map)['group'], isTrue);
    prefs.sound = false;
    final before = calls.length;
    await prefs.alert();
    await prefs.alert(group: true);
    expect(calls.length, before);
    prefs.dispose();
  });

  for (final video in [false, true]) {
    testWidgets('incoming ${video ? 'video' : 'audio'} ring follows the master bell and stops on disposal', (t) async {
      final prefs = AppPreferences();
      final signals = fixture.Signals()
        ..incoming = [CallRef(fixture.peer, 'call0001', incoming: true, video: video)];
      await t.pumpWidget(PreferencesScope(
        preferences: prefs,
        child: MaterialApp(home: Scaffold(body: CallsFeature(
          api: fixture.PhoneApi(), isActive: () => true, onLocked: () {}, onFailure: (_) {},
          signaling: signals, mediaFactory: fixture.Media.new,
        ))),
      ));
      await t.pumpAndSettle();
      expect(calls.where((c) => c.method == 'ringtone').last.arguments, {'kind': video ? 'video' : 'incoming'});
      prefs.sound = false;
      prefs.changed();
      await t.pumpAndSettle();
      expect(calls.last.arguments, {'kind': 'none'});
      prefs.sound = true;
      prefs.changed();
      await t.pumpAndSettle();
      expect(calls.last.arguments, {'kind': video ? 'video' : 'incoming'});
      await t.pumpWidget(const SizedBox.shrink());
      await t.pump();
      expect(calls.last.arguments, {'kind': 'none'});
      prefs.dispose();
    });
  }

  testWidgets('outgoing video uses dial tone and stops it when the peer accepts', (t) async {
    final prefs = AppPreferences(), signals = fixture.Signals();
    final controller = CallsController();
    await t.pumpWidget(PreferencesScope(preferences: prefs,
      child: MaterialApp(home: Scaffold(body: CallsFeature(
        api: fixture.PhoneApi(), controller: controller, isActive: () => true,
        onLocked: () {}, onFailure: (_) {}, signaling: signals, mediaFactory: fixture.Media.new,
      ))),
    ));
    await t.pumpAndSettle();
    await controller.start(fixture.peer, video: true);
    await t.pumpAndSettle();
    expect(calls.last.arguments, {'kind': 'outgoing'});
    signals.signal('accept');
    await t.pumpAndSettle();
    expect(calls.last.arguments, {'kind': 'none'});
    await t.pumpWidget(const SizedBox.shrink());
    await t.pump();
    prefs.dispose();
  });
}
