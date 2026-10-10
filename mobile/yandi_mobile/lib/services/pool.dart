import 'dart:async';

/// Выполняет [task] для 0..count-1, не больше [concurrency] одновременно. Первая ошибка останавливает выдачу новых задач и пробрасывается
/// после завершения уже запущенных (никакой «потерянной» ошибки и висящих задач). Порядок завершения произвольный.
Future<void> runPool(int count, int concurrency, Future<void> Function(int index) task) async {
  var next = 0;
  Object? failure;
  StackTrace? failureTrace;
  Future<void> worker() async {
    while (failure == null) {
      final i = next++;
      if (i >= count) return;
      try {
        await task(i);
      } catch (e, st) {
        failure ??= e;
        failureTrace ??= st;
      }
    }
  }
  await Future.wait(List.generate(concurrency < 1 ? 1 : concurrency, (_) => worker()));
  if (failure != null) Error.throwWithStackTrace(failure!, failureTrace ?? StackTrace.current);
}

/// Упорядоченное окно: стартует задачи [fetch] с опережением на [ahead] кусков, а результаты отдаёт строго по порядку 0..count-1
/// (чтобы расшифровка, контрольная сумма и запись на диск шли по порядку, а скачивание — заранее и параллельно).
Stream<T> orderedPrefetch<T>(int count, int ahead, Future<T> Function(int index) fetch) async* {
  final window = <Future<T>>[];
  var started = 0;
  void fill() {
    while (started < count && window.length < ahead) {
      final f = fetch(started++);
      f.ignore(); // ошибку заберёт тот, кто дойдёт до этого куска; иначе она «утекла» бы как необработанная
      window.add(f);
    }
  }
  fill();
  for (var i = 0; i < count; i++) {
    final value = await window.removeAt(0);
    fill();
    yield value;
  }
}
