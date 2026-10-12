import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_desktop/domain/file_repository.dart';
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
    final file = (await t.runAsync(() => File('${temp.path}/chosen.bin').writeAsBytes([1, 2, 3])))!;
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
    await m1.click(t, 'Выбрать файл');
    expect(find.text('chosen.bin'), findsOneWidget);
    await m1.click(t, 'Убрать файл');
    expect(find.text('chosen.bin'), findsNothing);
    await m1.click(t, 'Выбрать файл');
    api.reject = true;
    await t.ensureVisible(find.text('Отправить файл'));
    await t.runAsync(() async {
      await t.tap(find.text('Отправить файл'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await t.pumpAndSettle();
    expect(find.textContaining('превышен допустимый размер'), findsOneWidget);
    expect(find.text('chosen.bin'), findsOneWidget);
    api.reject = false;
    await t.runAsync(() async {
      await t.tap(find.text('Отправить файл'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await t.pumpAndSettle();
    expect(find.text('chosen.bin'), findsNothing);
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
      await m1.click(t, 'Проверить файл');
      await m1.click(t, 'Предпросмотр');
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
}
