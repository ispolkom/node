import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/replay_ledger.dart';

void main() {
  test('live claim passes queued history while writes stay serialized and fair',
      () async {
    final dir = await Directory.systemTemp.createTemp('yandi-replay-priority-');
    final entered = Completer<void>(), release = Completer<void>();
    final writes = <String>[];
    var inside = 0, maxInside = 0;
    final ledger = ReplayLedger(File('${dir.path}/ledger'),
        open: (s) async => s.isEmpty ? '{}' : s,
        seal: (s) async {
          inside++;
          if (inside > maxInside) maxInside = inside;
          writes.add((jsonDecode(s) as Map).keys.last as String);
          if (writes.length == 1) {
            entered.complete();
            await release.future;
          }
          inside--;
          return s;
        });
    final exp = DateTime.now().millisecondsSinceEpoch + 60000;
    final work = <Future<bool>>[ledger.claim('history-0', exp)];
    await entered.future;
    for (var i = 1; i <= 30; i++) {
      work.add(ledger.claim('history-$i', exp));
    }
    for (var i = 0; i < 8; i++) {
      work.add(ledger.claim('live-$i', exp, urgent: true));
    }
    release.complete();
    await Future.wait(work);
    expect(writes.take(6),
        ['history-0', 'live-0', 'live-1', 'live-2', 'live-3', 'history-1']);
    expect(maxInside, 1);
    final restarted = ReplayLedger(ledger.file);
    expect(await restarted.claim('live-0', exp, urgent: true), isFalse);
    expect(await restarted.claim('history-30', exp), isFalse);
    await dir.delete(recursive: true);
  });
  test(
      'failed urgent storage write cannot poison queue or accept corrupt ledger',
      () async {
    final dir = await Directory.systemTemp.createTemp('yandi-replay-error-');
    final file = File('${dir.path}/ledger');
    await file.writeAsString('corrupted');
    final ledger = ReplayLedger(file);
    final exp = DateTime.now().millisecondsSinceEpoch + 60000;
    await expectLater(
        ledger.claim('live', exp, urgent: true), throwsFormatException);
    await expectLater(ledger.claim('history', exp), throwsFormatException);
    expect(await file.readAsString(), 'corrupted');
    await dir.delete(recursive: true);
  });
}
