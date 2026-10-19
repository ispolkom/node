import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/services/prep_tracker.dart';

void main() {
  test('одинаковые имена разных файлов не мешают друг другу, а повтор того же источника распознаётся', () {
    final t = PrepTracker();
    final a = t.start('p', 'report.pdf', '/cache/a/report.pdf');
    final b = t.start('p', 'report.pdf', '/cache/b/report.pdf');
    expect(t.labels('p'), ['report.pdf', 'report.pdf']);
    expect(t.hasSource('p', '/cache/a/report.pdf'), isTrue);
    expect(t.hasSource('p', '/cache/c/report.pdf'), isFalse);
    expect(t.hasSource('other', '/cache/a/report.pdf'), isFalse);
    expect(t.done('p', a), isTrue);
    expect(t.labels('p'), ['report.pdf'], reason: 'снята только своя запись');
    expect(t.hasSource('p', '/cache/b/report.pdf'), isTrue);
    expect(t.done('p', b), isTrue);
    expect(t.labels('p'), isEmpty);
  });

  test('повторное снятие и чужой номер ничего не меняют: старая отправка не снимает подготовку новой', () {
    final t = PrepTracker();
    final a = t.start('p', 'x.mp4', '/c/x.mp4');
    expect(t.done('p', a), isTrue); // onPrepared первой операции
    final b = t.start('p', 'x.mp4', '/c/x.mp4'); // вторая операция с тем же именем и путём началась
    expect(t.done('p', a), isFalse, reason: 'finally первой операции не должен снять запись второй');
    expect(t.labels('p'), ['x.mp4']);
    expect(t.done('p', 9999), isFalse);
    expect(t.done('nobody', b), isFalse);
    expect(t.done('p', b), isTrue);
    expect(t.labels('p'), isEmpty);
  });

  test('копией выбора владеют все читатели: стирает последний, отказ повтора и очистка экрана не затирают чужой источник', () {
    final c = CopyOwners();
    c.acquire('/cache/report.pdf'); // операция A
    c.acquire('/cache/report.pdf'); // повтор B (будет отклонён)
    expect(c.inUse('/cache/report.pdf'), isTrue);
    expect(c.release('/cache/report.pdf'), isFalse, reason: 'B отпустил, A ещё читает');
    expect(c.inUse('/cache/report.pdf'), isTrue, reason: 'очистка экрана выбора в это время не должна стирать');
    c.acquire('/cache/report.pdf'); // C принят после onPrepared A
    expect(c.release('/cache/report.pdf'), isFalse, reason: 'A закончила, C ещё читает');
    expect(c.release('/cache/report.pdf'), isTrue, reason: 'последний отпустил — можно стирать');
    expect(c.inUse('/cache/report.pdf'), isFalse);
    expect(c.release('/cache/other.bin'), isTrue, reason: 'лишнее освобождение безопасно');
  });
}
