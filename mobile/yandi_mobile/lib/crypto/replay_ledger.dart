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
  static final _normal = <Future<void> Function()>[];
  static final _urgent = <Future<void> Function()>[];
  static bool _draining = false;
  static void _enqueue(Future<void> Function() work, {bool urgent = false}) {
    (urgent ? _urgent : _normal).add(work);
    if (_draining) return;
    _draining = true;
    scheduleMicrotask(() async {
      var burst = 0;
      try {
        while (_normal.isNotEmpty || _urgent.isNotEmpty) {
          // At most four live operations before pending history gets a turn.
          final live = _urgent.isNotEmpty && (_normal.isEmpty || burst < 4);
          final next = (live ? _urgent : _normal).removeAt(0);
          burst = live ? burst + 1 : 0;
          await next();
        }
      } finally {
        _draining = false;
      }
    });
  }

  /// Переписать файл в зашифрованном виде (старый открытый журнал шифруется сразу при входе, а не при следующем сообщении).
  Future<void> reseal() {
    final result = Completer<void>();
    _enqueue(() async {
      try {
        if (seal != null && open != null && await file.exists()) {
          final text = await file.readAsString();
          final json = await open!(text);
          final entries = jsonDecode(json) as Map<String, dynamic>;
          if (entries.values.any((v) => v is! int))
            throw StateError('Corrupt replay ledger');
          if (json == text) {
            // был открытым
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

  /// Видели ли уже этот номер (без записи). Нужен, чтобы занимать номер только после того, как письмо долговечно обработано
  /// (сохранено): иначе сбой сохранения терял бы сообщение — повтор отвергался бы как «уже виденное».
  Future<bool> seen(String id, {int? nowMs}) {
    final result = Completer<bool>();
    _enqueue(() async {
      try {
        final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
        final Map<String, dynamic> m = await file.exists()
            ? jsonDecode(open != null
                ? await open!(await file.readAsString())
                : await file.readAsString()) as Map<String, dynamic>
            : (open != null
                ? jsonDecode(await open!('')) as Map<String, dynamic>
                : {});
        if (m.values.any((v) => v is! int))
          throw StateError('Corrupt replay ledger');
        final exp = m[id];
        result.complete(exp is int && exp >= now);
      } catch (e, st) {
        result.completeError(e, st);
      }
    });
    return result.future;
  }

  /// [urgent] is for authenticated live envelopes, after signature and payload verification.
  /// It changes queue order only; the same durable replay transaction still applies.
  Future<bool> claim(String id, int expiresMs,
      {int? nowMs, bool urgent = false}) {
    final result = Completer<bool>();
    _enqueue(() async {
      try {
        final now = nowMs ?? DateTime.now().millisecondsSinceEpoch;
        final Map<String, dynamic> seen = await file.exists()
            ? jsonDecode(open != null
                ? await open!(await file.readAsString())
                : await file.readAsString()) as Map<String, dynamic>
            : (open != null
                ? jsonDecode(await open!('')) as Map<String, dynamic>
                : {});
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
        await temp.writeAsString(
            seal != null ? await seal!(jsonEncode(seen)) : jsonEncode(seen),
            flush: true);
        await temp.rename(file.path);
        if (seal != null) await afterWrite?.call();
        result.complete(true);
      } catch (e, st) {
        result.completeError(e, st);
      }
    }, urgent: urgent);
    return result.future;
  }
}
