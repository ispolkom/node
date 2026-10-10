import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Reload under a process-wide lock: reconnects and inbox/socket share durable IDs.
/// On corruption/write error/capacity exhaustion: fail closed, never reset or evict live IDs.
class ReplayLedger {
  final File file;
  ReplayLedger(this.file);
  static Future<void> _tail = Future.value();

  Future<bool> claim(String id, int expiresMs, {int? nowMs}) {
    final result = Completer<bool>();
    _tail = _tail.then((_) async {
      try {
        final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
        final Map<String, dynamic> seen = await file.exists()
            ? jsonDecode(await file.readAsString()) as Map<String, dynamic>
            : {};
        if (seen.values.any((v) => v is! int))
          throw StateError('Corrupt replay ledger');
        seen.removeWhere((_, v) => (v as int) < now);
        if (seen.containsKey(id)) {
          result.complete(false);
          return;
        }
        if (seen.length >= 100000 || expiresMs < now)
          throw StateError('Replay ledger full or envelope expired');
        seen[id] = expiresMs;
        final temp = File('${file.path}.tmp');
        await temp.writeAsString(jsonEncode(seen), flush: true);
        await temp.rename(file.path);
        result.complete(true);
      } catch (e, st) {
        result.completeError(e, st);
      }
    });
    return result.future;
  }
}
