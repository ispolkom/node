import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Reload under a process-wide lock: reconnects and inbox/socket share durable IDs.
/// On corruption/write error/capacity exhaustion: fail closed, never reset or evict live IDs.
class ReplayLedger {
  final File file;
  /// Шифрование файла журнала (в нём номера собеседников): [seal] — перед записью, [open] — после чтения (может принять и старый открытый
  /// вид, но порчу обязан отвергнуть исключением: журнал не сбрасывается и не «лечится»). Пустая строка означает отсутствие файла:
  /// callback обязан проверить, что это первый запуск, и вернуть пустой JSON только в этом случае. Без них файл открытый (тесты).
  final Future<String> Function(String json)? seal;
  final Future<String> Function(String stored)? open;
  /// После успешной записи зашифрованного журнала (для метки «открытый вид больше не принимать»).
  final Future<void> Function()? afterWrite;
  ReplayLedger(this.file, {this.seal, this.open, this.afterWrite});
  static Future<void> _tail = Future.value();

  /// Переписать файл в зашифрованном виде (старый открытый журнал шифруется сразу при входе, а не при следующем сообщении).
  Future<void> reseal() {
    final result = Completer<void>();
    _tail = _tail.then((_) async {
      try {
        if (seal != null && open != null && await file.exists()) {
          final text = await file.readAsString();
          final json = await open!(text);
          final entries = jsonDecode(json) as Map<String, dynamic>;
          if (entries.values.any((v) => v is! int)) throw StateError('Corrupt replay ledger');
          if (json == text) {   // был открытым
            final temp = File('${file.path}.tmp');
            await temp.writeAsString(await seal!(json), flush: true);
            await temp.rename(file.path);
          }
          // Аутентифицированный encrypted-файл мог пережить отказ записи метки: дочиниваем её и без нового reseal.
          await afterWrite?.call();
        } else if (!await file.exists()) {
          await open?.call('');
        }
        result.complete();
      } catch (e, st) {
        result.completeError(e, st);
      }
    });
    return result.future;
  }

  Future<bool> claim(String id, int expiresMs, {int? nowMs}) {
    final result = Completer<bool>();
    _tail = _tail.then((_) async {
      try {
        final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
        final Map<String, dynamic> seen = await file.exists()
            ? jsonDecode(open != null ? await open!(await file.readAsString()) : await file.readAsString()) as Map<String, dynamic>
            : (open != null ? jsonDecode(await open!('')) as Map<String, dynamic> : {});
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
        await temp.writeAsString(seal != null ? await seal!(jsonEncode(seen)) : jsonEncode(seen), flush: true);
        await temp.rename(file.path);
        if (seal != null) await afterWrite?.call();
        result.complete(true);
      } catch (e, st) {
        result.completeError(e, st);
      }
    });
    return result.future;
  }
}
