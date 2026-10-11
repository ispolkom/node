# Handoff: signed-release R1–R9, 2026-10-11

Рабочая копия `/mnt/ssd480/wt/codex`, ветка `codex/updater`.
Исходный HEAD: `e9dbda5`. Push не выполнялся; `~/node` и ветка `main` не менялись.
Файл `REVIEW_signed_release_2026-10-11.md` отсутствует в этой рабочей копии и соседних
worktree. Основание исправлений — формулировки R1–R9 из задания, проверенные по коду.

## Что исправлено

| Находка | Исправление | Регрессионная проверка |
| --- | --- | --- |
| R1 | CLI sign/verify читают manifest ≤1 MiB, signature ≤256 B. Проверка размера до чтения, ограничение limit+1 при чтении; обычный файл подтверждается fstat, O_NOFOLLOW/O_NONBLOCK исключают ссылки и зависание на FIFO. | `r1_bounded_metadata_rejects_sparse_symlink_fifo_and_directory`; CLI `r1_cli_bounds_manifest_before_reading_or_requesting_phrase`: раньше большой manifest доходил до ввода фразы, теперь отказ до него. |
| R2 | Общие RELEASE_BIN_DIR/REL для Cargo, запуска и архива. JSON Cargo подтверждает фактический каталог двух executable. Хэши запоминаются после сборки; сверяются до упаковки и перед подписью, сравниваются также байты внутри архива. | Python `test_r2_packaged_identity_is_checked`, `test_r2_actual_cargo_outputs_cannot_be_stale_other_directory`: подмена готового бинарника/другой output directory отвергаются. |
| R3 | `--check` требует готовый доверенный инструмент и существующий закреплённый ключ. Нет cargo build, создания каталога выпуска, якоря, SEQ или архива; несовместим с unsigned. | Python `test_r3_check_never_builds_or_creates_anchor`: отсутствующий anchor не создаётся, существующий проверяется с запрещённой настоящей сборкой. |
| R4 | Эксклюзивный flock на весь signed-сценарий. SEQ — строгий canonical decimal u64, без арифметической интерпретации shell. Резерв до подписи: tempfile, fsync файла, replace, fsync каталога. Переполнение и пропавший SEQ при существующем ключе вызывают отказ. Сбой расходует номер, повтор не допускается. | Python `test_r4_seq_is_strict_durable_and_never_reused`, `test_r4_signing_holds_exclusive_lock`: неверный SEQ/ссылка не переписываются; последовательные резервы различны; build stub проверяет, что конкурирующий lock недоступен. |
| R5 | ReleaseDir открывает компоненты каталога через openat/O_DIRECTORY/O_NOFOLLOW и удерживает dirfd. Обычные файлы открываются относительно него, fstat и хэш используют тот же fd. Локальный updater закрепляет каталог на всю загрузку; проверяет приватный снимок, распаковывает его, а не исходные пути. | `r5_pinned_directory_and_fd_survive_path_replacement`: после подмены пути каталога читаются исходные байты, хэш исходного fd сохранён; ссылка на каталог отвергается. Также тесты R1 и updater на tampering. |
| R6 | Raw phrase, NFC/whitespace buffers и Argon2 seed — Zeroizing. Raw строки уничтожаются сразу после нормализации. Ошибки ввода возвращаются через scope с Drop, только затем CLI завершает процесс. Нормализация без Vec слов. | `r6_normalized_phrase_is_zeroizing` фиксирует типы Zeroizing фразы/реального KDF seed и проверяет обнуление; CLI unit `r6_phrase_errors_return_through_zeroizing_scope` проверяет ошибочные пути. Удаление защищённых типов ломает compile-time часть теста. |
| R7 | Нормализация v1 встроена в KDF: NFC ICU4X 2.2.0, затем Unicode whitespace → одиночный пробел. Убрана отдельная Python NFC-обработка shell. Соль и production Argon2 неизменны. | `r7_nfc_vectors_and_legacy_nfc_key_compatibility`: composed/decomposed пары, три фиксированных независимых libargon2 seed-вектора, сравнение с прежним KDF для NFC-фразы. |
| R8 | CLI verify/update требует явный --min-seq (0 для первой установки); --current-seq остаётся alias. Updater требует seq > max(min_seq, stored max_seq), включая после rollback. Seq/max_seq уже записываются с fsync внутри поколения, активируемого атомарным rename current; эта схема сохранена и проверена. Документация больше не обещает защиту без min_seq и не разрешает повтор. | `r8_repeated_seq_is_refused_after_install_and_rollback`; CLI `r8_cli_requires_explicit_minimum_for_verify_and_update`. Existing tests SIGKILL до commit и retry, high-water после rollback. |
| R9 | Единые parse_manifest/validate_manifest для построения, sign, verify и проверки файлов. Нет дубликатов имён; SHA-256/key_id и подпись — lowercase hex. Проверяются format/product/version/seq/состав. CLI sign проверяет структуру и файлы до ввода фразы, key_id — до подписи. | `r9_validator_rejects_duplicates_uppercase_and_invalid_before_signing`; CLI `r9_cli_refuses_invalid_manifest_before_phrase_and_signature_write`. Неверные структуры тест дополнительно подписывает напрямую через Ed25519, чтобы проверить отказ верификатора независимо от sign. |

## Совместимость ключей

Прежний `node_release.sh` уже делал NFC через Python перед прежним KDF. Для уже NFC
фраз соль, параметры и байты KDF сохранены; это проверяется синтетическим legacy-вектором.
Ключи выпусков, закреплённых через прежний скрипт с обычными NFC-символами, не ротируются.
Существующий public key не заменяется: несовпадение вызывает отказ.

**Прямой прежний API/CLI с decomposed Unicode теперь выводит другой ключ.** Это явно
описанное изменение: ранее API расходился со скриптом. Для такого нестандартного якоря
нужна явная миграция владельцем, автоматической перепривязки нет. Версия Unicode в ICU
может также отличаться от Python владельца для недавно назначенных символов; сравнение
с уже закреплённым ключом остаётся обязательным и блокирует незаметную смену ключа.
Боевой якорь/фраза здесь не использовались; проверить конкретную фразу владельца нельзя
без нарушения задания. NFC и KDF проверены синтетическими векторами, не боевым секретом.

## Проверки

Все cargo-команды: `nice -n 15`, `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/mnt/ssd480/wt/codex/target-updater`,
`XDG_DATA_HOME=/tmp/codex_rf_xdg`, `--offline`, proxy-переменные удалены.
Для NFC использован существующий cache; ничего из сети не скачивалось.

- Python regression suite: 5/5 passed; настоящая сборка и фраза не нужны.
- `bash -n tools/node_release.sh tools/release_build_env.sh`: passed.
- Целевые release tests: 9/9 passed (до добавления фиксированных KDF-векторов;
  финальные векторы входят в полный прогон).
- Целевые updater tests: 9/9 passed, включая SIGKILL/retry и повтор seq после rollback.
- Целевые CLI: 3/3 integration + 1/1 unit passed.
- `git diff --check`: passed.
- Полный `cargo test --offline -- --test-threads=1` выполнен **ровно один раз** на финальном коде: exit 0; **524 passed, 0 failed, 13 ignored**, 27 test/doc suites.
  Библиотека: 497/497 passed за 199.33 s; все новые R1/R5/R6/R7/R8/R9 Rust-тесты прошли.
  Лог: `/tmp/codex_rf_full.log`; targeted логи: `/tmp/codex_rf_{release,updater,cli}.log`.
  Предсуществующие ignored-тесты не включались дополнительно. В сборке остаются
  предсуществующие warnings вне задания; ошибок компиляции в полном прогоне нет.

Предварительный `cargo test --no-run` застал редактирование API во время компиляции и
отказал из-за несогласованного временного состояния `lower_hex`; финальная CLI-сборка
успешна. Это не полный тестовый прогон. Предварительный cargo check для ICU потребовал
отключить ненужную default utf16 feature, которой нет в offline-cache; используется
compiled_data, остальные необходимые зависимости уже были доступны.

## Что не сделано и почему

- Исходный файл ревью не восстановлен: его содержимое отсутствует; исправлены именно
  девять явно перечисленных требований, а не предположенные дополнительные замечания.
- Не подписывался боевой выпуск, не вводилась и не проверялась фраза владельца, не
  создавались боевые public key/SEQ. Это прямое ограничение задания.
- Не повторялись две полные release-сборки для воспроизводимости: требуемая проверка —
  cargo test; прежние unsigned-хэши в документации явно помечены как дорефакторинговые.
- Zeroizing защищает Rust-буферы при обычном выходе и unwinding. Bash-ввод/pipe и
  внутреннее состояние криптобиблиотек не дают гарантии стирания всех копий при SIGKILL
  или core dump. Shell не имеет Zeroizing; переписывание всего интерактивного ввода и
  аварийной памяти не заявляется как часть исправления R6.
- Каталог установки, release state и доверенный инструмент должны оставаться под
  контролем владельца. Защита от другого процесса того же владельца, способного
  заменить сам инструмент, поколение/состояние установки или удалить SEQ/lock, не
  заявляется. fsync/rename рассчитаны на локальную Linux-ФС, не сетевую ФС.
- Закрытая среда, сетевой транспорт ноды, systemd и работающий узел не изменялись.

## Поимённые локальные коммиты

- `7a6ca7b` fix(release R1): bound metadata reads and reject links and special files
- `c115a61` fix(release R5): pin directories and hash opened regular file descriptors
- `5017ae8` fix(release R9): share canonical manifest validation before signing and verification
- `7d34564` fix(release R6): zeroize phrase buffers and Argon2 seed on all return paths
- `896ab9e` fix(release R2): use one artifact directory and compare packaged binaries before signing
- `90496ea` fix(release R3): make check read-only and require prebuilt trusted tool
- `ba652b7` fix(release R4): serialize signing and durably reserve strict nonreusable sequence numbers
- `4f2c9d9` fix(release R7): version NFC normalization inside key derivation and preserve script keys
- `4f50814` fix(updater R8): require explicit minimum and persist strict sequence across rollback
- `795d569` test(release R4 R9): reject lost sequence state and noncanonical signatures
- `373875a` test(release R1 R6 R7 R8 R9): add CLI regressions and independent NFC KDF vectors
- `f7cc777` fix(release R2): verify actual Cargo outputs and retain build identity through signing
- `96ed290` fix(release R4 R6): shorten raw phrase lifetime and reject wrong key before reserving seq
