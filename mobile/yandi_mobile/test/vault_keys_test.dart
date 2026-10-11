import 'dart:convert';
import 'package:flutter_test/flutter_test.dart';
import 'package:yandi_mobile/crypto/vault_keys.dart';

const _t = VaultKdf.test;
const _strong = ['мой', 'Тайный', 'первый', 'слово', 'ёлка'];   // 5 полей

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
    // 25 знаков в одном поле тоже годится
    await _make(MemoryVaultBackend(), phrase: ['а' * 25]);
    final b = MemoryVaultBackend();
    await _make(b, phrase: null);
    await expectLater(VaultKeys.recoverWithPhrase(b, _strong, 'новый-пароль'), throwsA(isA<VaultNoRecovery>()));
    final b2 = MemoryVaultBackend();
    await _make(b2, phrase: List.filled(20, ''));
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
    await expectLater(VaultKeys.recoverWithPhrase(b, ['Тайный', 'мой', 'первый', 'слово', 'ёлка'], 'новый-пароль'), throwsA(isA<VaultWrongSecret>()));
    final r = await VaultKeys.recoverWithPhrase(b, ['  МОЙ ', 'тайный', 'ПЕРВЫЙ', 'слово', 'елка'], 'новый-пароль');
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
    expect(VaultKeys.normalizePhrase(['А', '', 'б']).startsWith('а||б|'), isTrue);
    expect(VaultKeys.normalizePhrase(['а', 'б']), isNot(VaultKeys.normalizePhrase(['', 'а', 'б'])));
    expect(VaultKeys.normalizePhrase(['Ёж  Ёлка']).split('|').first, 'еж елка');
  });
}
