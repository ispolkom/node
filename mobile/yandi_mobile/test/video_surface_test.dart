import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:video_player/video_player.dart';
import 'package:yandi_mobile/widgets/video_surface.dart';

void main() {
  testWidgets('tap plays, controls hide, tap shows then pauses, fullscreen icon', (t) async {
    final value = ValueNotifier(const VideoPlayerValue(duration: Duration(minutes: 2), isInitialized: true));
    var plays = 0, pauses = 0, full = 0;
    await t.pumpWidget(MaterialApp(
      home: Scaffold(
        body: VideoSurface(
          video: const SizedBox(width: 100, height: 60),
          value: value,
          onPlay: () {
            plays++;
            value.value = value.value.copyWith(isPlaying: true);
          },
          onPause: () {
            pauses++;
            value.value = value.value.copyWith(isPlaying: false);
          },
          onSeek: (_) {},
          onToggleFullscreen: () => full++,
        ),
      ),
    ));
    // на паузе — белый треугольник; тап по видео запускает
    expect(find.byKey(const ValueKey('video-play')), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('video-tap')));
    await t.pump();
    expect(plays, 1);
    expect(find.byKey(const ValueKey('video-pause')), findsOneWidget);
    // через 4 секунды управление скрывается (значка полного экрана нет)
    await t.pump(const Duration(seconds: 5));
    expect(find.byKey(const ValueKey('video-fullscreen')), findsNothing);
    expect(find.byKey(const ValueKey('video-pause')), findsNothing);
    // тап показывает управление, второй — пауза
    await t.tap(find.byKey(const ValueKey('video-tap')));
    await t.pump();
    expect(find.byKey(const ValueKey('video-fullscreen')), findsOneWidget);
    expect(pauses, 0);
    await t.tap(find.byKey(const ValueKey('video-tap')));
    await t.pump();
    expect(pauses, 1);
    expect(find.byKey(const ValueKey('video-play')), findsOneWidget);
    // на паузе управление не прячется
    await t.pump(const Duration(seconds: 10));
    expect(find.byKey(const ValueKey('video-fullscreen')), findsOneWidget);
    await t.tap(find.byKey(const ValueKey('video-fullscreen')));
    expect(full, 1);
  });
}
