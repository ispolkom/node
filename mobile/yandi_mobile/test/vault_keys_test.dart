import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';

const _t = VaultKdf.test;
const _strong = ['Тайный', 'первый', 'слово', 'ёлочка'];   // 4 поля по 5+ знаков

Future<VaultKeys> _make(MemoryVaultBackend b, {List<String>? phrase = _strong, String pw = 'пароль-6'}) =>
    VaultKeys.create(b, password: pw, phraseFields: phrase, pwKdf: _t, phKdf: _t);

void main() {
  test('создание: пароль открывает тот же ключ, неверный — нет, файл не содержит ключа', () async {
    final b = MemoryVaultBackend();
    final v = await _make(b);
    final key = await v.subKey('db');
    v.lock();
    expect(v.unlocked, isFalse);
    expect(() => v.subKey('db'), throwsStateError);

    final again = await VaultKeys.unlock(b, 'пароль-6');
    expect(await again.subKey('db'), key);
    expect(await again.subKey('files'), isNot(key), reason: 'для разных целей разные ключи');
    await expectLater(VaultKeys.unlock(b, 'пароль-7'), throwsA(isA<VaultWrongSecret>()));
    expect(b.blob!.contains(base64.encode(key)), isFalse);
  });

  test('политика: пароль от 6 знаков, слабая фраза отвергается, пустая фраза — без восстановления', () async {
    await expectLater(_make(MemoryVaultBackend(), pw: '12345'), throwsA(isA<VaultPolicyError>()));
    await expectLater(_make(MemoryVaultBackend(), phrase: ['привет', 'мир']), throwsA(isA<VaultPolicyError>()));
    await expectLater(_make(MemoryVaultBackend(), phrase: ['тайный', 'первый', 'слово', 'мир']), throwsA(isA<VaultPolicyError>()), reason: 'в каждом поле от 5 знаков');
    await expectLater(_make(MemoryVaultBackend(), phrase: ['тайный', 'ТАЙНЫЙ', 'слово', 'ёлочка']), throwsA(isA<VaultPolicyError>()), reason: 'поля разные');
    await _make(MemoryVaultBackend(), phrase: ['тайный', 'первый', 'слово', 'ёлочка']);
    final b = MemoryVaultBackend();
    await _make(b, phrase: null);
    await expectLater(VaultKeys.recoverWithPhrase(b, _strong, 'новый-пароль'), throwsA(isA<VaultNoRecovery>()));
    final b2 = MemoryVaultBackend();
    await _make(b2, phrase: List.filled(4, ''));
    await expectLater(VaultKeys.recoverWithPhrase(b2, _strong, 'новый-пароль'), throwsA(isA<VaultNoRecovery>()), reason: 'пустые поля = фразы нет');
  });

  test('существующее хранилище не перезаписывается', () async {
    final b = MemoryVaultBackend();
    await _make(b);
    await expectLater(_make(b, pw: 'другой-пароль'), throwsStateError);
  });

  test('после трёх неверных попыток — паузы, растущие вдвое; верный пароль после паузы сбрасывает счёт', () async {
    final b = MemoryVaultBackend();
    (await _make(b)).lock();
    for (var i = 1; i <= 3; i++) {
      await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>().having((e) => e.failures, 'n', i)));
    }
    await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>()), reason: 'четвёртая ещё проверяется и ставит паузу 5 с');
    await expectLater(VaultKeys.unlock(b, 'пароль-6'), throwsA(isA<VaultThrottled>()), reason: 'в паузе не проверяем и верный');
    b.now += 5001;
    await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>()));
    b.now += 5001;
    await expectLater(VaultKeys.unlock(b, 'пароль-6'), throwsA(isA<VaultThrottled>().having((e) => e.wait.inSeconds, 'wait', inInclusiveRange(4, 5))), reason: 'вторая пауза 10 с, из неё прошло 5');
    b.now += 10001;
    (await VaultKeys.unlock(b, 'пароль-6')).lock();
    // счёт сброшен: снова три свободные попытки
    await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>().having((e) => e.failures, 'n', 1)));
  });

  test('пауза не превышает 15 минут', () async {
    final b = MemoryVaultBackend();
    (await _make(b)).lock();
    for (var i = 0; i < 30; i++) {
      b.now += 2000000; // дольше любой паузы
      await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>()));
    }
    final left = int.parse(b.secure['vault_next_pw']!) - b.now;
    expect(left, lessThanOrEqualTo(900000));
  });

  test('попытка считается до проверки: оборванная (убитый процесс) всё равно в счёте', () async {
    final b = MemoryVaultBackend();
    (await _make(b)).lock();
    // имитируем прерванную попытку: счётчик уже поднят, результата нет
    b.secure['vault_fail_pw'] = '3';
    await expectLater(VaultKeys.unlock(b, 'нет'), throwsA(isA<VaultWrongSecret>().having((e) => e.failures, 'n', 4)));
  });

  test('восстановление по фразе: тот же ключ, новый пароль, старый не работает; регистр, пробелы и ё/е не мешают, а позиция важна', () async {
    final b = MemoryVaultBackend();
    final v = await _make(b);
    final key = await v.subKey('db');
    v.lock();

    // другие позиции тех же слов — не та фраза
    await expectLater(VaultKeys.recoverWithPhrase(b, ['первый', 'Тайный', 'слово', 'ёлочка'], 'новый-пароль'), throwsA(isA<VaultWrongSecret>()));
    final r = await VaultKeys.recoverWithPhrase(b, ['  ТАЙНЫЙ ', 'первый', 'СЛОВО', 'елочка'], 'новый-пароль');
    expect(await r.subKey('db'), key);
    r.lock();
    await expectLater(VaultKeys.unlock(b, 'пароль-6'), throwsA(isA<VaultWrongSecret>()));
    expect(await (await VaultKeys.unlock(b, 'новый-пароль')).subKey('db'), key);
    await expectLater(VaultKeys.recoverWithPhrase(b, _strong, '123'), throwsA(isA<VaultPolicyError>()));
  });

  test('смена пароля: нужен старый, ключ данных тот же', () async {
    final b = MemoryVaultBackend();
    final v = await _make(b);
    final key = await v.subKey('db');
    await expectLater(v.changePassword('не тот', 'новый-пароль'), throwsA(isA<VaultWrongSecret>()));
    await v.changePassword('пароль-6', 'новый-пароль');
    v.lock();
    expect(await (await VaultKeys.unlock(b, 'новый-пароль')).subKey('db'), key);
    await expectLater(VaultKeys.unlock(b, 'пароль-6'), throwsA(isA<VaultWrongSecret>()));
  });

  test('файл, унесённый с телефона без секрета Keystore, не открывается даже верным паролем', () async {
    final b = MemoryVaultBackend();
    (await _make(b)).lock();
    final stolen = MemoryVaultBackend()..blob = b.blob;   // у вора только файл
    await expectLater(VaultKeys.unlock(stolen, 'пароль-6'), throwsA(isA<VaultWrongSecret>()));
    // а замок «фраза» открывается и без Keystore (так восстанавливаются на другом телефоне)
    final r = await VaultKeys.recoverWithPhrase(stolen, _strong, 'новый-пароль');
    expect(r.unlocked, isTrue);
  });

  test('повреждённый или чужой файл не перезаписывается и не принимается', () async {
    final b = MemoryVaultBackend();
    await expectLater(VaultKeys.unlock(b, 'x'), throwsA(isA<VaultCorrupt>()));
    b.blob = '{мусор';
    await expectLater(VaultKeys.unlock(b, 'x'), throwsA(isA<VaultCorrupt>()));
    expect(b.blob, '{мусор');
    b.blob = jsonEncode({'v': 2});
    await expectLater(VaultKeys.unlock(b, 'x'), throwsA(isA<VaultCorrupt>()));
  });

  test('подмена обёрнутого ключа отвергается, удаление ключей делает данные недоступными', () async {
    final b = MemoryVaultBackend();
    (await _make(b)).lock();
    final m = jsonDecode(b.blob!) as Map<String, dynamic>;
    final w = base64.decode(m['pw']['wrap'] as String)..[20] ^= 1;
    m['pw']['wrap'] = base64.encode(w);
    b.blob = jsonEncode(m);
    await expectLater(VaultKeys.unlock(b, 'пароль-6'), throwsA(isA<VaultWrongSecret>()));

    final c = MemoryVaultBackend();
    (await _make(c)).lock();
    await VaultKeys.destroy(c);
    expect(c.secure, isEmpty);
    await expectLater(VaultKeys.unlock(c, 'пароль-6'), throwsA(isA<VaultCorrupt>()));
  });

  test('нормализация фразы: позиция сохраняется, пустые поля занимают место', () {
    expect(VaultKeys.normalizePhrase(['А', '', 'б']), 'а||б|');
    expect(VaultKeys.normalizePhrase(['а', 'б']), isNot(VaultKeys.normalizePhrase(['', 'а', 'б'])));
    expect(VaultKeys.normalizePhrase(['Ёж  Ёлка']).split('|').first, 'еж елка');
  });

  test('слова с «|» внутри не сливаются с соседними полями (R12-4)', () async {
    final a = ['alpha|bravo', 'charlie', 'delta', 'echoo'], b2 = ['alpha', 'bravo|charlie', 'delta', 'echoo'];
    expect(VaultKeys.samePhrase(a, b2), isFalse);
    expect(VaultKeys.canonicalPhrase(a), isNot(VaultKeys.canonicalPhrase(b2)));
    final be = MemoryVaultBackend();
    (await VaultKeys.create(be, password: 'пароль-6', phraseFields: a, pwKdf: _t, phKdf: _t)).lock();
    await expectLater(VaultKeys.recoverWithPhrase(be, b2, 'новый-пароль'), throwsA(isA<VaultWrongSecret>()));
    expect((await VaultKeys.recoverWithPhrase(be, a, 'новый-пароль')).unlocked, isTrue);
    // не четыре поля — отказ
    await expectLater(VaultKeys.create(MemoryVaultBackend(), password: 'пароль-6', phraseFields: ['тайный', 'первый', 'слово'], pwKdf: _t, phKdf: _t), throwsA(isA<VaultPolicyError>()));
  });

  test('хранилище со старым способом записи слов продолжает открываться (совместимость)', () async {
    final be = MemoryVaultBackend();
    final v = await VaultKeys.create(be, password: 'пароль-6', phraseFields: _strong, pwKdf: _t, phKdf: _t, legacyPhraseEncoding: true);
    final key = await v.subKey('db');
    v.lock();
    expect(jsonDecode(be.blob!)['ph']['enc'], isNull);
    final r = await VaultKeys.recoverWithPhrase(be, _strong, 'новый-пароль');
    expect(await r.subKey('db'), key);
  });

  test('параллельные неверные попытки считаются по очереди, пауза включается (R12-2)', () async {
    final be = MemoryVaultBackend();
    (await _make(be)).lock();
    final results = await Future.wait([
      for (var i = 0; i < 12; i++)
        VaultKeys.unlock(be, 'нет-$i').then<Object>((_) => 'открыто', onError: (Object e) => e),
    ]);
    final wrong = results.whereType<VaultWrongSecret>().map((e) => e.failures).toList()..sort();
    expect(wrong.length + results.whereType<VaultThrottled>().length, 12);
    expect(wrong, [1, 2, 3, 4], reason: 'после четвёртой неверной включилась пауза, остальные получили её, а не бесплатные попытки');
    expect(be.secure['vault_fail_pw'], '4');
  });

  test('возвращённый старый файл ключей (с прежним паролем) отвергается (R12-1)', () async {
    final be = MemoryVaultBackend();
    final v = await _make(be);
    final old = be.blob!;
    await v.changePassword('пароль-6', 'новый-пароль');
    v.lock();
    be.blob = old;   // откат файла
    await expectLater(VaultKeys.unlock(be, 'пароль-6'), throwsA(isA<VaultCorrupt>()));
    await expectLater(VaultKeys.recoverWithPhrase(be, _strong, 'ещё-пароль'), throwsA(isA<VaultCorrupt>()));
    // честный файл после смены открывается
    final be2 = MemoryVaultBackend();
    final v2 = await _make(be2);
    await v2.changePassword('пароль-6', 'новый-пароль');
    v2.lock();
    expect((await VaultKeys.unlock(be2, 'новый-пароль')).unlocked, isTrue);
  });

  test('стёртый счётчик попыток не сбрасывает паузу (R12-3)', () async {
    final be = MemoryVaultBackend();
    (await _make(be)).lock();
    for (var i = 0; i < 4; i++) {
      await expectLater(VaultKeys.unlock(be, 'нет'), throwsA(isA<VaultWrongSecret>()));
    }
    be.secure.remove('vault_fail_pw');
    be.secure.remove('vault_next_pw');
    // нет ни счётчика, ни паузы, но метка есть: это нарушение, а не новая жизнь
    await expectLater(VaultKeys.unlock(be, 'нет'), throwsA(isA<VaultThrottled>()));
    await expectLater(VaultKeys.unlock(be, 'пароль-6'), throwsA(isA<VaultThrottled>()), reason: 'пауза снова есть');
  });

  test('старое хранилище без поколения и метки открывается и получает их', () async {
    final be = MemoryVaultBackend();
    (await _make(be)).lock();
    final m = jsonDecode(be.blob!) as Map<String, dynamic>;
    m.remove('gen');
    be.blob = jsonEncode(m);
    be.secure.remove('vault_gen');
    be.secure.remove('vault_cnt');
    be.secure.remove('vault_fail_pw');
    expect((await VaultKeys.unlock(be, 'пароль-6')).unlocked, isTrue);
    expect(be.secure['vault_gen'], '0');
    expect(be.secure['vault_cnt'], '1');
  });

  test('изменённый файл с завышенным поколением не поднимает метку: исправная копия после этого открывается', () async {
    final be = MemoryVaultBackend();
    (await _make(be)).lock();
    final good = be.blob!;
    final m = jsonDecode(good) as Map<String, dynamic>;
    m['gen'] = 50;
    (m['pw'] as Map<String, dynamic>)['wrap'] = base64.encode(List.filled(60, 1));   // замок не раскроется
    be.blob = jsonEncode(m);
    await expectLater(VaultKeys.unlock(be, 'пароль-6'), throwsA(isA<VaultWrongSecret>()));
    expect(be.secure['vault_gen'], '1', reason: 'метка не поднята по файлу, который не открылся');
    be.blob = good;
    be.now += 100000;
    expect((await VaultKeys.unlock(be, 'пароль-6')).unlocked, isTrue, reason: 'исходная исправная копия не стала «устаревшей»');
  });
}
