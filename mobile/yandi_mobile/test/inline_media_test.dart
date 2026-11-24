import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/video_frames.dart';
import 'package:yandi_mobile/theme.dart';
import 'package:yandi_mobile/widgets/inline_media.dart';

void main() {
  testWidgets('аудио в сообщении: круглая кнопка и время, без «Открыть»', (t) async {
    await t.pumpWidget(MaterialApp(
      theme: AppTheme.dark,
      home: Scaffold(
        body: InlineAudio(path: '/x/voice.yfe', size: 1000, loadBytes: (_) async => Uint8List(10)),
      ),
    ));
    expect(find.byKey(const ValueKey('audio-play')), findsOneWidget);
    expect(find.byKey(const ValueKey('audio-seek')), findsOneWidget);
    expect(find.text('0:00'), findsOneWidget);
    expect(find.byIcon(Icons.play_arrow), findsOneWidget);
    expect(find.text('Открыть'), findsNothing);
  });

  test('кадр видео берётся один раз и держится в памяти', () async {
    VideoFrames.clear();
    var calls = 0;
    VideoFrames.grabber = (path, size, mime) async {
      calls++;
      return Uint8List.fromList([1, 2, 3]);
    };
    addTearDown(() {
      VideoFrames.grabber = null;
      VideoFrames.clear();
    });
    final a = await VideoFrames.of('/v/1.yfe', 100, 'video/mp4');
    final b = await VideoFrames.of('/v/1.yfe', 100, 'video/mp4');
    expect(a, [1, 2, 3]);
    expect(b, [1, 2, 3]);
    expect(calls, 1);
    expect(VideoFrames.cached('/v/1.yfe'), isNotNull);
  });

  test('блокировка во время извлечения кадра: поздний кадр не попадает в кеш и не отдаётся', () async {
    VideoFrames.clear();
    final gate = Completer<Uint8List?>();
    VideoFrames.grabber = (path, size, mime) => gate.future;
    addTearDown(() {
      VideoFrames.grabber = null;
      VideoFrames.clear();
    });
    final pending = VideoFrames.of('/v/late.yfe', 100, 'video/mp4');
    VideoFrames.clear(); // приложение заперто
    gate.complete(Uint8List.fromList([9, 9, 9]));
    expect(await pending, isNull);
    expect(VideoFrames.cached('/v/late.yfe'), isNull);
  });

  testWidgets('видео в сообщении: кадр и белый треугольник «играть»', (t) async {
    VideoFrames.clear();
    VideoFrames.grabber = (p, s, m) async => null;
    addTearDown(() {
      VideoFrames.grabber = null;
      VideoFrames.clear();
    });
    await t.pumpWidget(MaterialApp(
      theme: AppTheme.dark,
      home: Scaffold(body: InlineVideo(path: '/v/2.yfe', size: 100, mime: 'video/mp4')),
    ));
    await t.pumpAndSettle();
    expect(find.byKey(const ValueKey('video-inline')), findsOneWidget);
    expect(find.byKey(const ValueKey('video-inline-play')), findsOneWidget);
  });

  test('одновременно играет только одно вложение', () {
    final a = Object(), b = Object();
    PlaybackHub.claim(a);
    expect(PlaybackHub.owner.value, a);
    PlaybackHub.claim(b);
    expect(PlaybackHub.owner.value, b);
    PlaybackHub.release(a);
    expect(PlaybackHub.owner.value, b);
    PlaybackHub.release(b);
    expect(PlaybackHub.owner.value, isNull);
  });
}
