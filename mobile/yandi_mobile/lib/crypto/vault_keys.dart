import 'dart:async';
import 'dart:convert';
import 'dart:isolate';
import 'dart:math';
import 'dart:typed_data';
import 'package:cryptography/cryptography.dart';

/// Ядро «закрытой среды»: один случайный ключ данных (DK), которым шифруется всё содержимое приложения (база, файлы).
/// Сам DK на диск не попадает, лежит только в обёрнутом виде, под двумя независимыми замками:
///   • замок «пароль»: Argon2id(пароль, соль) вместе с секретом, хранящимся в Android Keystore (`hw`): файл, унесённый с телефона,
///     без этого секрета не открывается, а перебор короткого пароля возможен только на самом телефоне;
///   • замок «фраза» (необязательный): Argon2id(позиционная фраза, соль), только для восстановления забытого пароля.
/// Проверка пароля — это проверка метки AES-GCM при раскрытии DK: неверный пароль не даёт ключа, а не «похожий» ключ.
/// Неверные попытки считаются ДО проверки (убить процесс посреди попытки не помогает) и замедляются: после третьей — паузы.

/// Что хранит ядро вне себя: один файл с обёрнутыми ключами и значения защищённого хранилища (Keystore).
abstract class VaultBackend {
  Future<String?> readBlob();
  Future<void> writeBlob(String text);
  Future<String?> secureRead(String key);
  Future<void> secureWrite(String key, String value);
  Future<void> secureDelete(String key);
  int nowMs();
}

/// Для тестов и временных сред: всё в памяти, часы подставляются.
class MemoryVaultBackend implements VaultBackend {
  String? blob;
  final Map<String, String> secure = {};
  int now = 1000000;
  @override
  Future<String?> readBlob() async => blob;
  @override
  Future<void> writeBlob(String text) async => blob = text;
  @override
  Future<String?> secureRead(String key) async => secure[key];
  @override
  Future<void> secureWrite(String key, String value) async => secure[key] = value;
  @override
  Future<void> secureDelete(String key) async => secure.remove(key);
  @override
  int nowMs() => now;
}

/// Параметры Argon2id (хранятся в файле: потом их можно усилить, не ломая прежних хранилищ).
class VaultKdf {
  final int memoryKiB, iterations, parallelism;
  const VaultKdf(this.memoryKiB, this.iterations, this.parallelism);
  Map<String, dynamic> toJson() => {'m': memoryKiB, 't': iterations, 'p': parallelism};
  factory VaultKdf.fromJson(Map<String, dynamic> j) => VaultKdf(j['m'] as int, j['t'] as int, j['p'] as int);

  /// Пароль вводят при каждом запуске: быстрее, но за ним ещё и секрет Keystore.
  static const password = VaultKdf(32768, 3, 2);
  /// Фразу вводят при создании и восстановлении: тяжелее, она единственная защита замка «фраза».
  static const phrase = VaultKdf(65536, 4, 2);
  /// Только для тестов.
  static const test = VaultKdf(8, 1, 1);
}

/// Пароль короче минимума или фраза слишком слабая.
class VaultPolicyError implements Exception {
  final String message;
  VaultPolicyError(this.message);
  @override
  String toString() => message;
}

/// Пароль или фраза не подошли; [failures] — подряд неверных попыток.
class VaultWrongSecret implements Exception {
  final int failures;
  VaultWrongSecret(this.failures);
  @override
  String toString() => 'Неверный пароль или фраза (попыток подряд: $failures)';
}

/// Слишком много неверных попыток: подождать.
class VaultThrottled implements Exception {
  final Duration wait;
  VaultThrottled(this.wait);
  @override
  String toString() => 'Слишком много попыток, подождите ${wait.inSeconds} с';
}

/// Фразу при создании не вводили: забытый пароль восстановить нельзя.
class VaultNoRecovery implements Exception {
  @override
  String toString() => 'Фраза восстановления не задана';
}

/// Файл ключей повреждён или не наш: не перезаписываем его, а сообщаем.
class VaultCorrupt implements Exception {
  final String why;
  VaultCorrupt(this.why);
  @override
  String toString() => 'Хранилище ключей повреждено: $why';
}

class VaultKeys {
  VaultKeys._(this._b);
  final VaultBackend _b;
  Uint8List? _dk;
  int _generation = 0;

  static const minPasswordLength = 6;
  /// Фраза: ровно 4 поля, в каждом не короче 5 знаков, поля разные. Это частная переписка, а не оборона государства: слов хватает,
  /// чтобы вскрывать каждого пользователя по отдельности было невыгодно (замок «фраза» стоит за тяжёлым Argon2id).
  static const phraseFieldCount = 4;
  static const minPhraseFieldChars = 5;
  /// После скольких подряд неверных попыток начинаются паузы.
  static const freeAttempts = 3;

  static const _kHw = 'vault_hw';
  static const _kFail = 'vault_fail_pw';
  static const _kFailPh = 'vault_fail_ph';
  static const _kNext = 'vault_next_pw';
  static const _kNextPh = 'vault_next_ph';
  /// Поколение файла ключей (растёт при смене пароля и восстановлении) — копия в защищённом хранилище: возвращённый старый файл
  /// ключей (с прежним паролем) отвергается.
  static const _kGen = 'vault_gen';
  /// Метка «этот файл уже был заверен»: после неё файл без `gt` — подделка/откат, а не прежний формат.
  static const _kGt = 'vault_gt';
  /// Метка «счётчики попыток заведены»: если метка есть, а счётчика нет — его стёрли, это считается нарушением и наказывается паузой.
  static const _kCnt = 'vault_cnt';

  /// Попытки пароля, восстановления и смены идут строго по очереди: параллельные вызовы не читают один и тот же счётчик и не запускают
  /// несколько тяжёлых хэшей одновременно.
  static Future<void> _tail = Future.value();
  static Future<T> _serial<T>(Future<T> Function() op) {
    final r = Completer<T>();
    _tail = _tail.then((_) async {
      try {
        r.complete(await op());
      } catch (e, st) {
        r.completeError(e, st);
      }
    });
    return r.future;
  }

  bool get unlocked => _dk != null;

  /// Только для проверок: прежний тег поколения (до заверения обёрток), каким его писали старые версии.
  Future<String> legacyTagForTest(int gen) => _genTagV1(_dk!, gen);

  /// Подключ для конкретной цели (ключ базы, ключ файлов …): из DK одним HKDF, чтобы сам DK нигде не использовался напрямую.
  Future<Uint8List> subKey(String purpose) async {
    final dk = _dk;
    if (dk == null) throw StateError('хранилище заблокировано');
    final k = await Hkdf(hmac: Hmac.sha256(), outputLength: 32)
        .deriveKey(secretKey: SecretKey(dk), nonce: utf8.encode('yandi-vault-sub:v1'), info: utf8.encode(purpose));
    return Uint8List.fromList(await k.extractBytes());
  }

  /// Стереть ключ из памяти (блокировка экрана, уход в фон).
  void lock() {
    _generation++;
    final dk = _dk;
    if (dk != null) {
      dk.fillRange(0, dk.length, 0);
      _dk = null;
    }
  }

  // ── Создание, открытие, восстановление ────────────────────────────────────────────────────────────────────────────

  static Future<bool> exists(VaultBackend b) async {
    final t = await b.readBlob();
    return t != null && t.isNotEmpty;
  }

  /// Создать хранилище: пароль обязателен (≥ 6 знаков), фраза — по желанию (null или пустые поля — без восстановления).
  /// Существующее хранилище не перезаписывается.
  static Future<VaultKeys> create(
    VaultBackend b, {
    required String password,
    List<String>? phraseFields,
    VaultKdf pwKdf = VaultKdf.password,
    VaultKdf phKdf = VaultKdf.phrase,
    /// Только для проверки совместимости: старый способ записи слов (поля через `|`, был до исправления неоднозначности).
    bool legacyPhraseEncoding = false,
  }) async {
    if (await exists(b)) throw StateError('хранилище уже создано');
    if (password.runes.length < minPasswordLength) throw VaultPolicyError('Пароль не короче $minPasswordLength знаков');
    final hasPhrase = phraseFields != null && _normalizedList(phraseFields).any((x) => x.isNotEmpty);
    if (hasPhrase && (phraseFields.length != phraseFieldCount || !phraseStrongEnough(phraseFields))) {
      throw VaultPolicyError('Фраза: $phraseFieldCount разных слова, в каждом не меньше $minPhraseFieldChars знаков');
    }
    final phrase = !hasPhrase ? null : (legacyPhraseEncoding ? normalizePhrase(phraseFields) : canonicalPhrase(phraseFields));
    final rnd = Random.secure();
    Uint8List rb(int n) => Uint8List.fromList(List.generate(n, (_) => rnd.nextInt(256)));
    final dk = rb(32);
    final hw = rb(32);
    await b.secureWrite(_kHw, base64.encode(hw));
    final saltPw = rb(16), saltPh = rb(16);
    final kekPw = await _kekPassword(password, saltPw, pwKdf, hw);
    final wrapPw = await _wrap(dk, kekPw, 'pw');
    Uint8List? wrapPh;
    if (hasPhrase) wrapPh = await _wrap(dk, await _kekPhrase(phrase!, saltPh, phKdf), 'ph');
    final file = <String, dynamic>{
      'v': 1,
      'gen': 1,
      'pw': {'kdf': pwKdf.toJson(), 'salt': base64.encode(saltPw), 'wrap': base64.encode(wrapPw)},
      'ph': hasPhrase
          ? {'kdf': phKdf.toJson(), 'salt': base64.encode(saltPh), 'wrap': base64.encode(wrapPh!), if (!legacyPhraseEncoding) 'enc': 2}
          : null,
    };
    file['gt'] = await _genTag(dk, 1, file);   // заверены номер поколения и обе секции обёрток
    await b.writeBlob(jsonEncode(file));
    await b.secureWrite(_kGen, '1');
    await b.secureWrite(_kGt, '2');   // файл создан сразу заверенным: снять `gt` и поднять `gen` до первого входа уже нельзя
    await _resetCounters(b);
    return VaultKeys._(b).._dk = dk;
  }

  /// Открыть паролем. [VaultWrongSecret] — не подошёл, [VaultThrottled] — рано.
  static Future<VaultKeys> unlock(VaultBackend b, String password) => _serial(() => _unlock(b, password));

  static Future<VaultKeys> _unlock(VaultBackend b, String password) async {
    final f = await _readFile(b);
    await _checkGeneration(b, f);
    await _gate(b, _kFail, _kNext);
    final failures = await _bump(b, _kFail, _kNext);   // считаем ДО проверки
    final hw = await b.secureRead(_kHw);
    final pw = f['pw'] as Map<String, dynamic>;
    Uint8List? dk;
    if (hw != null) {
      final kek = await _kekPassword(password, base64.decode(pw['salt'] as String), VaultKdf.fromJson(pw['kdf'] as Map<String, dynamic>), base64.decode(hw));
      dk = await _unwrap(base64.decode(pw['wrap'] as String), kek, 'pw');
    }
    if (dk == null) throw VaultWrongSecret(failures);
    await _adoptGeneration(b, f, dk);
    await _resetCounters(b);
    return VaultKeys._(b).._dk = dk;
  }

  /// Забыли пароль: открыть фразой и сразу задать новый пароль.
  static Future<VaultKeys> recoverWithPhrase(VaultBackend b, List<String> phraseFields, String newPassword, {void Function()? beforeCommit}) =>
      _serial(() => _recover(b, phraseFields, newPassword, beforeCommit: beforeCommit));

  static Future<VaultKeys> _recover(VaultBackend b, List<String> phraseFields, String newPassword, {void Function()? beforeCommit}) async {
    final f = await _readFile(b);
    await _checkGeneration(b, f);
    final ph = f['ph'] as Map<String, dynamic>?;
    if (ph == null) throw VaultNoRecovery();
    if (newPassword.runes.length < minPasswordLength) throw VaultPolicyError('Пароль не короче $minPasswordLength знаков');
    if (phraseFields.length != phraseFieldCount) throw VaultPolicyError('Нужно $phraseFieldCount слова');
    await _gate(b, _kFailPh, _kNextPh);
    final failures = await _bump(b, _kFailPh, _kNextPh);
    // новые замки записывают слова однозначно (JSON); старые — как были (поля через «|»)
    final phrase = ph['enc'] == 2 ? canonicalPhrase(phraseFields) : normalizePhrase(phraseFields);
    final kek = await _kekPhrase(phrase, base64.decode(ph['salt'] as String), VaultKdf.fromJson(ph['kdf'] as Map<String, dynamic>));
    final dk = await _unwrap(base64.decode(ph['wrap'] as String), kek, 'ph');
    if (dk == null) throw VaultWrongSecret(failures);
    await _adoptGeneration(b, f, dk);
    try {
      beforeCommit?.call();
      await _setPassword(b, f, dk, newPassword, beforeCommit: beforeCommit);
      await _resetCounters(b);
      return VaultKeys._(b).._dk = dk;
    } catch (_) {
      dk.fillRange(0, dk.length, 0);
      rethrow;
    }
  }

  /// Сменить пароль (хранилище должно быть открыто; старый пароль проверяется заново).
  Future<void> changePassword(String oldPassword, String newPassword) async {
    if (newPassword.runes.length < minPasswordLength) throw VaultPolicyError('Пароль не короче $minPasswordLength знаков');
    final generation = _generation;
    void alive() {
      if (_dk == null || generation != _generation) throw StateError('смена пароля прервана блокировкой');
    }
    alive();
    await _serial(() async {
      alive();
      // Проверенный ключ принадлежит этой операции: блокировка сеанса не превращает его в нули.
      final check = await _unlock(_b, oldPassword);
      try {
        alive();
        await _setPassword(_b, await _readFile(_b), check._dk!, newPassword, beforeCommit: alive);
      } finally {
        check.lock();
      }
    });
  }

  /// Удалить все ключи (кнопка «Удалить всё»): после этого данные нечитаемы навсегда.
  static Future<void> destroy(VaultBackend b) => _serial(() async {
    await b.writeBlob('');
    for (final k in [_kHw, _kFail, _kFailPh, _kNext, _kNextPh, _kGen, _kGt, _kCnt]) {
      await b.secureDelete(k);
    }
  });

  // ── Фраза ──────────────────────────────────────────────────────────────────────────────────────────────────────────

  /// Каждое поле: обрезать, привести к нижнему регистру, «ё» → «е», пробелы внутри схлопнуть; позиция сохраняется (`|` между полями).
  /// Лишние поля сверх четырёх отбрасываются, недостающие — пустые.
  /// Так опечатки в регистре и «ё/е» не лишают человека данных.
  static String normalizePhrase(List<String> fields) {
    final f = List<String>.generate(phraseFieldCount, (i) => i < fields.length ? fields[i] : '');
    return f.map((s) => s.trim().toLowerCase().replaceAll('ё', 'е').replaceAll(RegExp(r'\s+'), ' ')).join('|');
  }

  /// Слова для ключа: JSON-массив ровно из четырёх нормализованных полей. Поля с «|» внутри (знак есть на нашей клавиатуре) не
  /// сливаются с соседними: `["a|b","c",…]` и `["a","b|c",…]` дают разные ключи.
  static String canonicalPhrase(List<String> fields) => jsonEncode(_normalizedList(fields));

  static List<String> _normalizedList(List<String> fields) => List<String>.generate(
      phraseFieldCount, (i) => (i < fields.length ? fields[i] : '').trim().toLowerCase().replaceAll('ё', 'е').replaceAll(RegExp(r'\s+'), ' '));

  /// Одинаковые ли слова (для проверки «повторите слова»), без путаницы с «|».
  static bool samePhrase(List<String> a, List<String> b) => canonicalPhrase(a) == canonicalPhrase(b);

  /// Все четыре поля заполнены, в каждом ≥ 5 знаков, и они не повторяют друг друга.
  static bool phraseStrongEnough(List<String> fields) {
    if (fields.length != phraseFieldCount) return false;
    final parts = _normalizedList(fields);
    return parts.every((s) => s.runes.length >= minPhraseFieldChars) && parts.toSet().length == parts.length;
  }

  // ── Внутреннее ─────────────────────────────────────────────────────────────────────────────────────────────────────

  static Future<Map<String, dynamic>> _readFile(VaultBackend b) async {
    final t = await b.readBlob();
    if (t == null || t.isEmpty) throw VaultCorrupt('файла нет');
    try {
      final m = jsonDecode(t) as Map<String, dynamic>;
      if (m['v'] != 1 || m['pw'] is! Map) throw VaultCorrupt('неизвестный формат');
      return m;
    } catch (e) {
      if (e is VaultCorrupt) rethrow;
      throw VaultCorrupt('не читается');
    }
  }

  static Future<void> _setPassword(VaultBackend b, Map<String, dynamic> f, Uint8List dk, String password, {void Function()? beforeCommit}) async {
    final rnd = Random.secure();
    final salt = Uint8List.fromList(List.generate(16, (_) => rnd.nextInt(256)));
    final hw = await b.secureRead(_kHw);
    final hwBytes = hw != null ? base64.decode(hw) : Uint8List.fromList(List.generate(32, (_) => rnd.nextInt(256)));
    beforeCommit?.call();
    if (hw == null) await b.secureWrite(_kHw, base64.encode(hwBytes));   // восстановление на другом телефоне: свой секрет Keystore
    final kdf = VaultKdf.fromJson((f['pw'] as Map<String, dynamic>)['kdf'] as Map<String, dynamic>);
    final kek = await _kekPassword(password, salt, kdf, hwBytes);
    f['pw'] = {'kdf': kdf.toJson(), 'salt': base64.encode(salt), 'wrap': base64.encode(await _wrap(dk, kek, 'pw'))};
    final gen = ((f['gen'] as int?) ?? 0) + 1;
    f['gen'] = gen;
    f['gt'] = await _genTag(dk, gen, f);
    beforeCommit?.call();   // после всех вычислений, непосредственно перед записью нового замка
    await b.writeBlob(jsonEncode(f));      // сначала файл, потом метка: оборвёт между ними — файл новее метки, это допустимо
    await b.secureWrite(_kGen, '$gen');
    await b.secureWrite(_kGt, '2');
  }

  /// Файл старее записанной метки поколения — его вернули из копии (с прежним паролем): отвергаем. Метки ещё нет (хранилище создано
  /// до этой защиты или метку стёрли) — принимаем и заводим по файлу. Против root, возвращающего и метку, без аппаратной защиты
  /// счётчика ничего не сделать (это ограничение, а не забытая проверка).
  static Future<void> _checkGeneration(VaultBackend b, Map<String, dynamic> f) async {
    final fg = (f['gen'] as int?) ?? 0;
    final sg = int.tryParse(await b.secureRead(_kGen) ?? '');
    if (sg != null && fg < sg) throw VaultCorrupt('файл ключей старее текущего: возвращена прежняя копия');
  }

  /// Поколение файла запоминается только после того, как замок в нём реально открылся: изменённый, но не проходящий проверку файл
  /// не может поднять метку и сделать исправную копию «устаревшей».
  /// Номер поколения в файле заверен ключом данных (`gt` = HMAC(DK, «gen:N»)): изменить только `gen` и поднять метку нельзя.
  /// Файл прежнего вида без заверения метку не поднимает (только заводит, когда её ещё нет) и заверяется при первом входе.
  static Future<void> _adoptGeneration(VaultBackend b, Map<String, dynamic> f, Uint8List dk) async {
    final fg = (f['gen'] as int?) ?? 0;
    final sg = int.tryParse(await b.secureRead(_kGen) ?? '');
    final tag = f['gt'];
    if (tag is String) {
      final v2 = await _genTag(dk, fg, f);
      final legacy = tag != v2 && tag == await _genTagV1(dk, fg) && await b.secureRead(_kGt) != '2';
      if (tag != v2 && !legacy) throw VaultCorrupt('поколение файла ключей не заверено: файл изменён');
      var migrated = !legacy;
      if (legacy) {
        // прежний тег заверял только номер поколения: один раз перезаверяем по-новому (с обёртками)
        f['gt'] = v2;
        try {
          await b.writeBlob(jsonEncode(f));
          migrated = true;
        } catch (_) {
          // запись не удалась (диск полон): файл остаётся с прежним тегом, а метка НЕ поднимается до версии 2 — иначе следующий вход
          // отверг бы исправный файл как «понижение формата». Миграция повторится при следующем входе.
        }
      }
      if (sg == null || fg > sg) await b.secureWrite(_kGen, '$fg');
      if (migrated) {
        await b.secureWrite(_kGt, '2');
      } else if (await b.secureRead(_kGt) == null) {
        await b.secureWrite(_kGt, '1');
      }
      return;
    }
    // файл без заверения допустим только как прежний формат; если заверенный файл уже был, удалённое `gt` — подмена
    if (await b.secureRead(_kGt) != null) throw VaultCorrupt('у файла ключей снято заверение поколения: файл изменён');
    // незаверенное поколение не может быть выше метки больше чем на оборванную запись (на 1): иначе это подъём «в долг»
    if (sg != null && fg > sg + 1) throw VaultCorrupt('поколение файла ключей выше записанной метки: файл изменён');
    if (sg == null) await b.secureWrite(_kGen, '$fg');
    try {
      f['gt'] = await _genTag(dk, fg, f);
      await b.writeBlob(jsonEncode(f));   // заверяем прежний файл (то же поколение)
    } catch (_) {}
  }

  /// Тег поколения v2: HMAC(DK) над номером поколения И обеими секциями обёрток (`pw`, `ph`: KDF, соль, обёртка). Старую (корректную)
  /// секцию пароля под текущий `gt` подставить нельзя: тег перестаёт сходиться.
  static Future<String> _genTag(Uint8List dk, int gen, Map<String, dynamic> f) async {
    final mac = await Hmac.sha256().calculateMac(utf8.encode('yandi-vault:gen:v2:$gen:${_canon(f['pw'])}:${_canon(f['ph'])}'), secretKey: SecretKey(dk));
    return base64.encode(mac.bytes);
  }

  /// Прежний тег (только номер поколения): принимается один раз, чтобы перезаверить файл по-новому.
  static Future<String> _genTagV1(Uint8List dk, int gen) async {
    final mac = await Hmac.sha256().calculateMac(utf8.encode('yandi-vault:gen:v1:$gen'), secretKey: SecretKey(dk));
    return base64.encode(mac.bytes);
  }

  /// Каноническая запись JSON-значения (ключи по порядку): одинаковая у файла, только что собранного и прочитанного заново.
  static String _canon(Object? v) {
    if (v is Map) {
      final keys = v.keys.map((k) => '$k').toList()..sort();
      return '{${keys.map((k) => '${jsonEncode(k)}:${_canon(v[k])}').join(',')}}';
    }
    if (v is List) return '[${v.map(_canon).join(',')}]';
    return jsonEncode(v);
  }

  /// Сколько неверных подряд. Метка заведена, а счётчика нет — его стёрли: засчитываем как несколько ошибок (с паузой).
  static Future<int> _count(VaultBackend b, String kFail) async {
    final v = int.tryParse(await b.secureRead(kFail) ?? '');
    if (v != null) return v;
    return (await b.secureRead(_kCnt)) == null ? 0 : freeAttempts + 3;   // нет метки — хранилище старого вида, счёт с нуля
  }

  static Future<void> _gate(VaultBackend b, String kFail, String kNext) async {
    if (await b.secureRead(_kCnt) == null) {
      // Старый формат или восстановление на новом устройстве: заводим ОБА счётчика до первой попытки.
      await _resetCounters(b);
      return;
    }
    final count = int.tryParse(await b.secureRead(kFail) ?? '');
    final storedNext = int.tryParse(await b.secureRead(kNext) ?? '');
    if (await b.secureRead(_kCnt) != null &&
        (count == null || count < 0 || (count > freeAttempts && (storedNext == null || storedNext <= 0)))) {
      // Повреждение или оборванная запись счётчиков обнаруживаются ДО KDF; эта попытка не проверяет секрет.
      final n = max(freeAttempts + 3, count ?? 0);
      await b.secureWrite(kFail, '$n');
      final wait = Duration(seconds: min(900, 5 * (1 << min(n - freeAttempts - 1, 10))));
      await b.secureWrite(kNext, '${b.nowMs() + wait.inMilliseconds}');
      throw VaultThrottled(wait);
    }
    final next = storedNext ?? 0;
    final left = next - b.nowMs();
    if (left > 0) throw VaultThrottled(Duration(milliseconds: left));
  }

  /// Подряд неверных попыток после этой; ставит паузу, если пора.
  static Future<int> _bump(VaultBackend b, String kFail, String kNext) async {
    final n = (await _count(b, kFail)) + 1;
    await b.secureWrite(kFail, '$n');
    await b.secureWrite(_kCnt, '1');
    if (n > freeAttempts) {
      final sec = min(900, 5 * (1 << min(n - freeAttempts - 1, 10)));   // 5, 10, 20 … до 15 минут
      await b.secureWrite(kNext, '${b.nowMs() + sec * 1000}');
    }
    return n;
  }

  static Future<void> _resetCounters(VaultBackend b) async {
    await b.secureWrite(_kFail, '0');
    await b.secureWrite(_kFailPh, '0');
    await b.secureWrite(_kCnt, '1');
    await b.secureDelete(_kNext);
    await b.secureDelete(_kNextPh);
  }

  static Future<SecretKey> _kekPassword(String password, Uint8List salt, VaultKdf kdf, List<int> hw) async {
    final stretched = await _argon(utf8.encode(password), salt, kdf);
    return _hkdf([...stretched, ...hw], 'yandi-vault-pw:v1');
  }

  static Future<SecretKey> _kekPhrase(String normalized, Uint8List salt, VaultKdf kdf) async {
    final stretched = await _argon(utf8.encode(normalized), salt, kdf);
    return _hkdf(stretched, 'yandi-vault-ph:v1');
  }

  static Future<SecretKey> _hkdf(List<int> ikm, String info) =>
      Hkdf(hmac: Hmac.sha256(), outputLength: 32).deriveKey(secretKey: SecretKey(ikm), nonce: utf8.encode('yandi-vault-kek'), info: utf8.encode(info));

  /// Argon2id в отдельном потоке выполнения: интерфейс не замирает на секунды.
  static Future<List<int>> _argon(List<int> secret, List<int> salt, VaultKdf kdf) =>
      Isolate.run(() => _argonSync(secret, salt, kdf.memoryKiB, kdf.iterations, kdf.parallelism));

  static Future<List<int>> _argonSync(List<int> secret, List<int> salt, int m, int t, int p) async {
    final k = await Argon2id(memory: m, parallelism: p, iterations: t, hashLength: 32).deriveKey(secretKey: SecretKey(secret), nonce: salt);
    return k.extractBytes();
  }

  static final _aes = AesGcm.with256bits();

  static Future<Uint8List> _wrap(Uint8List dk, SecretKey kek, String slot) async {
    final box = await _aes.encrypt(dk, secretKey: kek, aad: utf8.encode('yandi-vault:v1:$slot'));
    return Uint8List.fromList(box.concatenation());
  }

  static Future<Uint8List?> _unwrap(List<int> wrapped, SecretKey kek, String slot) async {
    try {
      final box = SecretBox.fromConcatenation(wrapped, nonceLength: 12, macLength: 16);
      return Uint8List.fromList(await _aes.decrypt(box, secretKey: kek, aad: utf8.encode('yandi-vault:v1:$slot')));
    } catch (_) {
      return null;
    }
  }
}
