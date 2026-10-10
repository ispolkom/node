import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/pool.dart';

void main() {
  test('runPool: все задачи выполнены, одновременно не больше предела', () async {
    var running = 0, peak = 0;
    final done = <int>[];
    await runPool(20, 4, (i) async {
      running++;
      if (running > peak) peak = running;
      await Future<void>.delayed(Duration(milliseconds: 5 + (i % 3) * 5));
      running--;
      done.add(i);
    });
    expect(done.toSet().length, 20);
    expect(peak, lessThanOrEqualTo(4));
    expect(peak, greaterThan(1));
  });

  test('runPool: ошибка пробрасывается, новые задачи не стартуют, запущенные дожидаются', () async {
    final started = <int>[];
    var finished = 0;
    await expectLater(
      runPool(50, 3, (i) async {
        started.add(i);
        await Future<void>.delayed(const Duration(milliseconds: 10));
        if (i == 5) throw StateError('boom');
        finished++;
      }),
      throwsA(isA<StateError>()),
    );
    expect(started.length, lessThan(50));
    expect(finished, started.length - 1); // все запущенные, кроме упавшей, доработали
  });

  test('orderedPrefetch: результаты строго по порядку, скачивание с опережением', () async {
    var inFlight = 0, peak = 0;
    final got = <int>[];
    await for (final v in orderedPrefetch<int>(12, 4, (i) async {
      inFlight++;
      if (inFlight > peak) peak = inFlight;
      await Future<void>.delayed(Duration(milliseconds: (12 - i) * 3)); // поздние кончают раньше ранних
      inFlight--;
      return i;
    })) {
      got.add(v);
    }
    expect(got, List.generate(12, (i) => i));
    expect(peak, greaterThan(1));
  });

  test('orderedPrefetch: ошибка куска доходит до потребителя в его очереди', () async {
    final got = <int>[];
    Object? err;
    try {
      await for (final v in orderedPrefetch<int>(8, 3, (i) async {
        if (i == 4) throw StateError('chunk 4');
        return i;
      })) {
        got.add(v);
      }
    } catch (e) {
      err = e;
    }
    expect(got, [0, 1, 2, 3]);
    expect(err, isA<StateError>());
  });
}
