import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/file_repository.dart';
import 'package:yandi_desktop/embedded_player.dart';
import 'package:yandi_desktop/media_player.dart';
import 'package:yandi_desktop/platform_files.dart';
import 'package:yandi_desktop/transport/api.dart';
import 'shell_m2_test.dart' as m2;
import 'widget_test.dart' as m1;

final png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==',
);

class FileApi extends m2.FeatureApi implements BinaryApi {
  final chunks = <Map<String, String>>[];
  final payloads = <List<int>>[];
  bool exists = false, reject = false;
  @override
  Future<Map<String, dynamic>> multipart(
    String path,
    Map<String, String> fields,
    String field,
    String filename,
    Uint8List bytes,
  ) async {
    if (reject) throw const ApiFailure('file_limit');
    chunks.add(Map.of(fields));
    payloads.add(List.of(bytes));
    if (path == '/api/profile/avatar') return {'status': 'success'};
    final received = int.parse(fields['chunk_index']!) + 1;
    return {
      'file_id': 'file_1',
      'received': received,
      'complete': received == int.parse(fields['total_chunks']!),
    };
  }

  @override
  Future<Uint8List> bytes(String path, {int limit = 8 * 1024 * 1024}) async =>
      Uint8List.fromList(png);
  @override
  Future<void> download(
    String path,
    File destination, {
    required int limit,
    required bool Function() isActive,
  }) async {
    if (isActive()) await destination.writeAsBytes(png);
  }

  @override
  Future<Map<String, dynamic>> request(
    String path, {
    Map<String, dynamic>? body,
    String? method,
  }) async {
    if (path.startsWith('/api/files/status/')) return {'exists': exists};
    return super.request(path, body: body, method: method);
  }
}

void main() {
  late Directory temp;
  setUp(() async {
    temp = await Directory.systemTemp.createTemp('m4-files-');
  });
  tearDown(() async {
    await temp.delete(recursive: true);
  });
  test(
    'sequential node chunks, real progress, one attachment and no retries',
    () async {
      final file = await File(
        '${temp.path}/тест файл.bin',
      ).writeAsBytes(List.generate(1401, (i) => i % 255));
      final api = FileApi(), progress = <int>[];
      final attachment = await FileRepository(api).upload(
        m2.alice,
        file,
        phone: false,
        isActive: () => true,
        progress: (done, total) {
          expect(total, 1401);
          progress.add(done);
        },
      );
      expect(api.payloads.map((p) => p.length), [700, 700, 1]);
      expect(api.chunks.first.containsKey('file_id'), false);
      expect(api.chunks.last['file_id'], 'file_1');
      expect(progress, [700, 1400, 1401]);
      expect(api.bodies.last['attachment'], attachment);
      api.reject = true;
      await expectLater(
        FileRepository(api).upload(
          m2.alice,
          file,
          phone: false,
          isActive: () => true,
          progress: (_, _) {},
        ),
        throwsA(isA<ApiFailure>()),
      );
      expect(api.payloads.length, 3);
    },
  );
  test(
    'phone upload uses one MiB chunks and one attachment after completion',
    () async {
      final api = FileApi();
      final file = await File(
        '${temp.path}/phone.bin',
      ).writeAsBytes(Uint8List(1024 * 1024 + 1));
      await FileRepository(api).upload(
        m2.alice,
        file,
        phone: true,
        isActive: () => true,
        progress: (_, _) {},
      );
      expect(api.payloads.map((b) => b.length), [1024 * 1024, 1]);
      expect(api.chunks.map((c) => c['total_chunks']), ['2', '2']);
      expect(api.bodies.length, 1);
    },
  );
  test('empty files and navigation cancellation never send metadata', () async {
    final api = FileApi(),
        file = await File('${temp.path}/empty').writeAsBytes([]);
    await expectLater(
      FileRepository(api).upload(
        m2.alice,
        file,
        phone: true,
        isActive: () => true,
        progress: (_, _) {},
      ),
      throwsA(isA<ApiFailure>().having((e) => e.kind, 'kind', 'empty_file')),
    );
    await file.writeAsBytes([1]);
    await expectLater(
      FileRepository(api).upload(
        m2.alice,
        file,
        phone: false,
        isActive: () => false,
        progress: (_, _) {},
      ),
      throwsA(isA<ApiFailure>()),
    );
    expect(api.chunks, isEmpty);
    expect(api.bodies, isEmpty);
  });
  testWidgets('file picker, cancel attachment, send and daemon rejection', (
    t,
  ) async {
    final file = (await t.runAsync(
      () => File('${temp.path}/chosen.txt').writeAsBytes([1, 2, 3]),
    ))!;
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      PlatformFiles.channel,
      (call) async => call.method == 'choose' ? file.path : null,
    );
    addTearDown(
      () => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        PlatformFiles.channel,
        null,
      ),
    );
    final api = FileApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.tap(find.byTooltip('Выбрать файл'));
    await t.pumpAndSettle();
    expect(find.text('chosen.txt'), findsOneWidget);
    await m1.click(t, 'Убрать файл');
    expect(find.text('chosen.txt'), findsNothing);
    await t.tap(find.byTooltip('Выбрать файл'));
    await t.pumpAndSettle();
    api.reject = true;
    await t.ensureVisible(find.text('Отправить файл'));
    await t.runAsync(() async {
      await t.tap(find.text('Отправить файл'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await t.pumpAndSettle();
    expect(find.textContaining('превышен допустимый размер'), findsOneWidget);
    expect(find.text('chosen.txt'), findsOneWidget);
    api.reject = false;
    await t.ensureVisible(find.text('Отправить файл'));
    await t.runAsync(() async {
      await t.tap(find.text('Отправить файл'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await t.pumpAndSettle();
    expect(find.text('chosen.txt'), findsNothing);
    expect(api.payloads.length, 1);
    await m1.stop(t);
  });
  testWidgets(
    'incoming availability watch, raster preview and explicit download',
    (t) async {
      final api = FileApi();
      api.historyRows.first['attachment'] = {
        'filename': 'pic.png',
        'size': png.length,
        'mime_type': 'image/png',
        'file_ref': {'file_id': 'file_1', 'total_chunks': 1},
      };
      final saved = File('${temp.path}/saved.png');
      t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        PlatformFiles.channel,
        (call) async => call.method == 'choose' ? saved.path : null,
      );
      addTearDown(
        () => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          PlatformFiles.channel,
          null,
        ),
      );
      await m2.open(t, api, 'Чат');
      await m1.click(t, 'Открыть чат');
      expect(find.text('Файл ещё не получен'), findsOneWidget);
      expect(find.text('Изменить сообщение'), findsNothing);
      api.exists = true;
      await m1.word(t, 'Проверить файл');
      await m1.word(t, 'Предпросмотр');
      expect(find.text('Файл доступен на узле'), findsOneWidget);
      await t.ensureVisible(find.text('Скачать'));
      await t.runAsync(() async {
        await t.tap(find.text('Скачать'));
        await Future<void>.delayed(const Duration(milliseconds: 100));
      });
      await t.pumpAndSettle();
      expect(await t.runAsync(saved.exists), true);
      await m1.stop(t);
    },
  );
  test('pairing codes: only the two known roles pass the path check', () {
    expect(validApiPath('/api/mobile/pairing'), true);
    expect(validApiPath('/api/mobile/pairing?role=user'), true);
    expect(validApiPath('/api/mobile/pairing?role=owner'), true);
    expect(validApiPath('/api/mobile/pairing?role=admin'), false);
    expect(validApiPath('/api/mobile/pairing?role=user&x=1'), false);
  });
  test(
    'encoded filenames allow unicode, reject traversal and executable URIs',
    () {
      expect(
        validApiPath(
          '/api/files/content/file_1/${Uri.encodeComponent('фото 1.png')}',
        ),
        true,
      );
      for (final name in ['..', '../file', 'a/b', 'a\\b', 'a\n']) {
        expect(
          validApiPath(
            '/api/files/content/file_1/${Uri.encodeComponent(name)}',
          ),
          false,
        );
      }
      expect(
        FileRepository.raster(Uint8List.fromList(utf8.encode('<svg><script>'))),
        false,
      );
    },
  );
  testWidgets('system file dialog focus loss does not wipe the chat', (
    t,
  ) async {
    final file = (await t.runAsync(
      () => File('${temp.path}/focus.txt').writeAsBytes([1]),
    ))!;
    final gate = Completer<String?>();
    t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      PlatformFiles.channel,
      (call) async => call.method == 'choose' ? gate.future : null,
    );
    addTearDown(
      () => t.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        PlatformFiles.channel,
        null,
      ),
    );
    final api = FileApi();
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.tap(find.byTooltip('Выбрать файл'));
    await t.pump();
    // Диалог забрал фокус: окно «неактивно», но содержимое остаётся.
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await t.pump();
    gate.complete(file.path);
    await t.pump();
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pump(const Duration(seconds: 1));
    await t.pumpAndSettle();
    expect(find.text('focus.txt'), findsOneWidget);
  });
  testWidgets('audio plays through loopback in memory, no player is reported', (
    t,
  ) async {
    final api = FileApi()..exists = true;
    api.historyRows.first['attachment'] = {
      'filename': 'song.mp3',
      'size': png.length,
      'mime_type': 'audio/mpeg',
      'file_ref': {'file_id': 'file_2', 'total_chunks': 1},
    };
    Uri? seen;
    final saved = MediaPlayerLauncher.launcher;
    MediaPlayerLauncher.launcher = (url, title) async {
      seen = url;
      return null;
    };
    addTearDown(() => MediaPlayerLauncher.launcher = saved);
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.ensureVisible(find.text('Слушать'));
    await t.runAsync(() async {
      await t.tap(find.text('Слушать'));
      await Future<void>.delayed(const Duration(milliseconds: 300));
    });
    await t.pump();
    await t.pump();
    expect(seen?.host, '127.0.0.1');
    expect(find.textContaining('Не найден проигрыватель'), findsOneWidget);
    await m1.stop(t);
  });

  testWidgets('встроенный проигрыватель получает адрес петли, внешний не запускается, после закрытия память освобождается', (t) async {
    final api = FileApi()..exists = true;
    api.historyRows.first['attachment'] = {
      'filename': 'clip.mov',
      'size': png.length,
      'mime_type': 'video/quicktime',
      'file_ref': {'file_id': 'file_3', 'total_chunks': 1},
    };
    Uri? shown;
    String? shownTitle;
    bool? shownVideo;
    final close = Completer<void>();
    final savedShow = EmbeddedPlayer.show;
    final savedEnabled = EmbeddedPlayer.enabled;
    final savedLauncher = MediaPlayerLauncher.launcher;
    var externalCalls = 0;
    EmbeddedPlayer.enabled = true;
    EmbeddedPlayer.show = (ctx, url, title, video) {
      shown = url;
      shownTitle = title;
      shownVideo = video;
      return close.future;
    };
    MediaPlayerLauncher.launcher = (url, title) async {
      externalCalls++;
      return null;
    };
    addTearDown(() {
      EmbeddedPlayer.show = savedShow;
      EmbeddedPlayer.enabled = savedEnabled;
      MediaPlayerLauncher.launcher = savedLauncher;
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.ensureVisible(find.text('Смотреть'));
    await t.runAsync(() async {
      await t.tap(find.text('Смотреть'));
      await Future<void>.delayed(const Duration(milliseconds: 300));
    });
    await t.pump();
    expect(shown?.host, '127.0.0.1');
    expect(shownTitle, 'clip.mov');
    expect(shownVideo, isTrue);
    expect(externalCalls, 0, reason: 'внешний проигрыватель не нужен, пока работает встроенный');
    expect(find.text('Остановить'), findsOneWidget, reason: 'карточка показывает, что идёт воспроизведение');
    // закрытие окна проигрывателя останавливает воспроизведение и сервер в памяти
    close.complete();
    await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 100)));
    await t.pump();
    expect(find.text('Остановить'), findsNothing);
    expect(find.text('Смотреть'), findsOneWidget);
    await m1.stop(t);
  });

  testWidgets('окно проигрывателя забирает фокус: пока оно открыто, чат не стирается, после закрытия правило снова действует', (t) async {
    final api = FileApi()..exists = true;
    api.historyRows.first['attachment'] = {
      'filename': 'clip.mov',
      'size': png.length,
      'mime_type': 'video/quicktime',
      'file_ref': {'file_id': 'file_4', 'total_chunks': 1},
    };
    final close = Completer<void>();
    final savedShow = EmbeddedPlayer.show;
    final savedEnabled = EmbeddedPlayer.enabled;
    EmbeddedPlayer.enabled = true;
    EmbeddedPlayer.show = (ctx, url, title, video) => close.future;
    addTearDown(() {
      EmbeddedPlayer.show = savedShow;
      EmbeddedPlayer.enabled = savedEnabled;
      PlaybackGuard.reset();
    });
    await m2.open(t, api, 'Чат');
    await m1.click(t, 'Открыть чат');
    await t.ensureVisible(find.text('Смотреть'));
    await t.runAsync(() async {
      await t.tap(find.text('Смотреть'));
      await Future<void>.delayed(const Duration(milliseconds: 300));
    });
    await t.pump();
    expect(PlaybackGuard.active, isTrue);
    // проигрыватель забрал фокус — чат остаётся
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await t.pump();
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pump(const Duration(seconds: 1));
    expect(find.text('Остановить'), findsOneWidget, reason: 'воспроизведение и чат на месте');
    // окно закрыто — защита снята, обычная потеря фокуса снова стирает содержимое
    close.complete();
    await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 100)));
    await t.pump();
    expect(PlaybackGuard.active, isFalse);
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await t.pump();
    expect(find.text('Смотреть'), findsNothing, reason: 'после закрытия проигрывателя потеря фокуса стирает чат как раньше');
    t.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await t.pump(const Duration(seconds: 1));
  });

  test('без libmpv встроенный проигрыватель выключен и оболочка использует внешний', () {
    expect(EmbeddedPlayer.enabled, isFalse, reason: 'в тестах библиотека не инициализируется');
  });

  test('все разрешённые политикой форматы видео и аудио узнаются: кнопки «Смотреть» и «Слушать» не пропадают для .mov/.mkv/.3gp/.amr', () {
    const video = {'a.mp4': 'video/mp4', 'a.MOV': 'video/quicktime', 'a.mkv': 'video/x-matroska', 'a.webm': 'video/webm', 'a.3gp': 'video/3gpp'};
    video.forEach((name, mime) => expect(FileRepository.mime(name), mime, reason: name));
    for (final n in ['a.mp3', 'a.m4a', 'a.aac', 'a.ogg', 'a.opus', 'a.wav', 'a.flac', 'a.amr']) {
      expect(FileRepository.mime(n).startsWith('audio/'), isTrue, reason: n);
    }
    expect(FileRepository.mime('a.pdf'), 'application/octet-stream');
  });
}
