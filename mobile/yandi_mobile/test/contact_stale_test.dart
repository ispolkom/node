import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/models/contact.dart';

void main() {
  final now = DateTime(2026, 10, 10, 12).millisecondsSinceEpoch;
  Contact dev({bool online = false, bool saved = false, int? last}) => Contact(
        peerId: 'p', displayName: 'd', online: online, isManual: saved, lastActivityMs: last ?? 0);

  test('устройство не в контактах, не в сети дольше суток — скрыто', () {
    expect(dev(last: now - const Duration(hours: 25).inMilliseconds).isStale(now), isTrue);
    expect(dev(last: now - const Duration(hours: 23).inMilliseconds).isStale(now), isFalse);
  });

  test('вернулось в сеть — снова на главном экране, хоть через полгода', () {
    final away = now - const Duration(days: 180).inMilliseconds;
    expect(dev(last: away).isStale(now), isTrue);
    expect(dev(online: true, last: away).isStale(now), isFalse);
  });

  test('контакты и неизвестное время активности не скрываются', () {
    final old = now - const Duration(days: 400).inMilliseconds;
    expect(dev(saved: true, last: old).isStale(now), isFalse);
    expect(dev(last: 0).isStale(now), isFalse);
  });
}
