# CODEX review result

> **Historical audit snapshot (9 October 2026).** The findings below describe the code examined on that date, not the current `main` branch. Some issues were subsequently changed or fixed. For current trust boundaries and open findings, read [`docs/CRYPTO_MAP.md`](docs/CRYPTO_MAP.md), [`docs/PRIVACY.md`](docs/PRIVACY.md), and [`docs/SECURITY_STATUS.md`](docs/SECURITY_STATUS.md). This internal AI-assisted review is not an independent security audit.

Дата проверки: 2026-10-09
Репозиторий: `/home/iam/node`
Объект: Rust-нода и Flutter-приложение YANDI
Правило: в рамках этой проверки исходный код не изменялся. Создан только этот отчёт.

## Краткий итог

Критические проблемы безопасности всё ещё присутствуют. Основная граница доверия мобильного клиента сейчас опирается на TLS/транспорт, а не на гарантированное E2E-шифрование и криптографически связанную идентичность устройства. Поэтому нельзя утверждать, что текущая реализация обеспечивает заявленную модель «E2E между телефонами».

Наиболее опасные пункты:

- обычная отправка сообщений допускает plaintext fallback;
- certificate pinning отключается пустым fingerprint;
- X25519-ключи принимаются без подписи и проверки принадлежности peer ID;
- session key сохраняется в открытом JSON;
- `E2EEncryption` в Rust является no-op;
- RawIP tunnel принимает TCP на `0.0.0.0` без аутентификации и лимитов;
- resume имеет legacy plaintext ACK и хранит секреты в файле.

## Подтверждено исправлено

### #3 — cleartext URL в Flutter-клиенте: ЧАСТИЧНО ИСПРАВЛЕНО

`mobile/yandi_mobile/lib/models/trusted_node.dart:24-29` формирует только `https://` и `wss://`. В `mobile/yandi_mobile/android/app/src/main/AndroidManifest.xml:30` установлен `android:usesCleartextTraffic="false"`. В Flutter-коде не обнаружены рабочие `http://` или `ws://` URL.

Это устраняет прямое использование cleartext-транспорта мобильным клиентом. Однако это не исправляет проблемы доверия к сертификату, описанные в #9, и не превращает транспорт в E2E.

### #1 — привязка resume к node ID: ИСПРАВЛЕНО В ОСНОВНОМ FAST-PATH

`src/netlayer/transport.rs:1008-1015` проверяет HMAC, соответствие `embedded_node_id` ключу клиента через `resume_node_matches` и сохранённое `node_id_hex`. Повторная проверка sender/payload присутствует в `src/netlayer/transport.rs:5454-5465`.

Осталась legacy-логика resume с plaintext ACK, поэтому защита идентичности не должна считаться полной защитой канала.

## Всё ещё актуально (с оценкой риска)

### #4 — bearer token в WebSocket URL: ЧАСТИЧНО ИСПРАВЛЕНО — высокий риск

Мобильный клиент передаёт токен заголовком в `mobile/yandi_mobile/lib/services/ws_service.dart:126-130` через `Authorization: Bearer ...`, а не в URL.

Но сервер по-прежнему принимает query-параметр `token` в `src/mobile_api.rs:344-354`. Поэтому любой старый или альтернативный клиент может оставить bearer token в URL, где он попадает в логи, прокси и диагностические трассы. Совместимость с query-token нужно удалить после контролируемой миграции.

### #9 — certificate pinning: ЧАСТИЧНО ИСПРАВЛЕНО — высокий риск

`mobile/yandi_mobile/lib/services/api_service.dart:241-249` при пустом fingerprint возвращает обычный `http.Client`, то есть pinning не применяется. В `mobile/yandi_mobile/lib/services/ws_service.dart:341-348` callback также возвращает `true`, если fingerprint пуст.

TLS включён, но пустая конфигурация фактически означает «доверять системному хранилищу сертификатов». Для production нужно отклонять пустой fingerprint, если режим pinning включён, и явно разделить development/test и release-политику.

### #8 — хранение мобильного token: ЧАСТИЧНО ИСПРАВЛЕНО — средний/высокий риск

Новые записи очищают token перед SQLite-записью: `mobile/yandi_mobile/lib/services/storage_service.dart:126-133`, а сам token сохраняется через secure storage. При загрузке выполняется перенос legacy-значения из SQLite в secure storage: `storage_service.dart:108-123`.

Проблема остаётся в схеме `storage_service.dart:41-57`: колонка `token TEXT` не удалена, а миграция lazy и очищает только строки, которые были загружены. Старые неиспользованные записи могут продолжать содержать bearer token. Нужна явная миграция всех строк, удаление колонки либо отдельный зашифрованный контейнер, а также ротация уже выданных токенов.

### #4/#10 — plaintext fallback для сообщений и служебных маркеров: ПОДТВЕРЖДЕНО — критический риск

`mobile/yandi_mobile/lib/services/ws_service.dart:253-274` при отсутствии ключа получателя выполняет:

```dart
payload = Uint8List.fromList(utf8.encode(text));
```

Обычный `sendMessage` продолжает отправку. Наличие отдельного `sendEncryptedOnly` в `ws_service.dart:276-290` не исправляет ситуацию: основные пути `app_state.dart:545`, `631`, `657` и `676-692` используют обычный `sendMessage`.

Из-за этого plaintext могут быть не только обычные сообщения, но и delivery/read receipts, edit-маркеры и другие служебные payload. Нужно fail-closed поведение: если ключ peer отсутствует или не подтверждён, отправка должна завершаться ошибкой и не сохраняться как успешно отправленная.

### #11 — неподписанные X25519-ключи: ПОДТВЕРЖДЕНО — высокий риск

`src/mobile_api.rs:484-489` возвращает `x25519_pub` без подписи. `src/mobile_api.rs:492-512` принимает `ed25519_pub` и `x25519_pub`, проверяя только декодирование и длину 32 байта. Связь между ключами, peer ID и владением ключом не проверяется.

Flutter получает ключ без криптографической проверки в `mobile/yandi_mobile/lib/services/api_service.dart:104-116`, а публикует оба ключа без signature/attestation в `api_service.dart:121-131`. Возможна подмена ключа узлом или атакующим внутри доверенной транспортной точки.

### #12 — нет AAD и replay protection в mobile E2E: ПОДТВЕРЖДЕНО — высокий риск

`mobile/yandi_mobile/lib/crypto/e2e_crypto.dart:7-15` задаёт формат ciphertext без message ID, sequence или epoch. `e2e_crypto.dart:35-43` шифрует AES-GCM без AAD, а `e2e_crypto.dart:73-78` расшифровывает без криптографического счётчика и replay window.

Локальная дедупликация сообщений не является криптографической защитой от повторной доставки. Нужны authenticated header/AAD, монотонный sequence или nonce registry и политика отбрасывания повторов/старых epoch.

### #15 — Rust media E2E: ПОДТВЕРЖДЕНО В КОДЕ — критический риск для этого пути

`src/media/session/stream.rs:117-130` создаёт `EphemeralSecret`, но сразу теряет секрет и возвращает только public key. `establish_shared_secret` фактически хэширует удалённый public key вместо выполнения X25519 ECDH. Это не совместное секрета вычисление и не обеспечивает E2E.

При этом мобильные звонки используют `flutter_webrtc`: `mobile/yandi_mobile/lib/services/call_service.dart:5,13,50-55`. WebRTC применяет DTLS-SRTP на уровне WebRTC, но это не исправляет дефект отдельного Rust media-пути и не доказывает сквозную аутентификацию peer identity.

### #16 — RawIP tunnel: ПОДТВЕРЖДЕНО — критический риск при включении команды

`src/netlayer/rawip_tunnel.rs:126-145` слушает `0.0.0.0:<port>` и принимает TCP. В `rawip_tunnel.rs:153-227` отсутствуют peer authentication, TLS/MAC, connection limit, idle timeout, byte quota и packet rate limit.

Это не обязательно запускается при обычном старте, но реальная команда `rawip-exit` включает tunnel (`src/netlayer/cli.rs:654-680`). При доступности порта любой сетевой клиент может использовать его как неаутентифицированный вход.

### #2 — session key в paired_clients.json: ПОДТВЕРЖДЕНО — высокий риск

`src/netlayer/pairing.rs:75-90` хранит `session_key_hex` как сериализуемое поле `SessionToken`; `pairing.rs:112-117` записывает ключ, а `pairing.rs:139-149` извлекает его. Файл хранения — `~/.yandi/paired_clients.json` (`pairing.rs:444-446`). Тесты persistence подтверждают сам факт сериализации.

Права файла, шифрование хранилища и безопасная ротация не являются частью этого формата. Компрометация файла даёт материал для восстановления encrypted channel.

### #19 — Rust E2EEncryption является no-op: ПОДТВЕРЖДЕНО — критический риск

`src/communication/encryption.rs:10-13` прямо называет структуру заглушкой. `encryption.rs:21-32` возвращает plaintext без изменений. Тест `encryption.rs:44-51` называется `test_e2e_noop` и не проверяет ни конфиденциальность, ни аутентичность.

Наличие transport encryption не может заменить заявленное E2E между конечными устройствами.

### #5 — Secure cookie: НЕ ПОДТВЕРЖДЕНО КАК ИСПРАВЛЕННО — средний риск

`src/web/auth.rs:623-644` формирует cookies с `HttpOnly`, `SameSite=Strict`, `Path` и `Max-Age`, но флаг `Secure` отсутствует. При ошибочной публикации HTTP endpoint cookie может передаваться по незашифрованному соединению.

### #7 — небезопасный default bind: ПОДТВЕРЖДЕНО — высокий риск конфигурации

`src/core/config.rs:66` задаёт WebSocket default `0.0.0.0:8443`, `config.rs:89` задаёт общий bind `0.0.0.0`. Дополнительные fallback sockets также слушают `0.0.0.0` в `src/netlayer/transport.rs:615-629`.

Это может быть оправдано для ноды, но безопасный default должен быть явным, с предупреждением/allowlist/firewall policy. Нельзя считать локально запущенную ноду автоматически локальной по уровню экспозиции.

### #13 — pairing endpoint rate limit: ЧАСТИЧНО — средний/высокий риск

`src/mobile_api.rs:291-308` ограничивает попытки через `PAIRING_MAX_FAILS` и срок действия кода, но в показанном endpoint нет IP/device rate limit, backoff или lockout по источнику. Один общий короткоживущий код может подвергаться online brute-force до исчерпания счётчика.

### #17 — TUN setup через внешние команды: ПОДТВЕРЖДЕНО — средний риск

`src/netlayer/tun_device.rs:601-628` собирает команды и запускает `ip` через `Command::new(parts[0]).args(...)`. Сейчас значения проходят через split и не являются shell-командой, что снижает риск shell injection, но действия выполняются с сетевыми привилегиями. Нужны строгая валидация IPv6/device name, отсутствие произвольных аргументов из удалённого ввода и минимизация privilege boundary.

### #18 — внешняя системная информация: ЧАСТИЧНО — средний риск приватности

Системные команды вызываются из `src/netlayer/node_introspection.rs:244-362`, а также из `external_ip.rs`, `interface_detector.rs` и `util/sysmon.rs`. Доступ к таким данным должен быть явно ограничен доверенным локальным запросам и минимальным набором полей. Иначе диагностика может раскрывать сетевую/системную информацию удалённому peer.

### #6 — документация о E2E: НЕ ИСПРАВЛЕНО — высокий риск вводящего в заблуждение утверждения

`mobile/yandi_mobile/lib/services/call_service.dart:13` описывает WebRTC как шифрование между телефонами, а обычная переписка при отсутствии recipient key уходит plaintext внутри транспорта (`ws_service.dart:253-265`). Документация и UI должны чётко различать transport TLS, WebRTC DTLS-SRTP и проверенное E2E с authenticated peer identity.

## Новые проблемы/регрессии

1. Исправление клиента для Authorization header не завершено миграцией сервера: query-token всё ещё принимается (`src/mobile_api.rs:344-354`).
2. В приложении есть безопасный по названию `sendEncryptedOnly`, но основные пути продолжают использовать fallback-capable `sendMessage`. Это создаёт ложное ощущение, что сообщения защищены.
3. Legacy resume допускает plaintext ACK (`src/netlayer/transport.rs:1027-1035`, комментарии и fallback в `transport.rs:5522-5525`), а старые токены могут не иметь session key.
4. Тест `test_e2e_noop` закрепляет отсутствие шифрования как допустимое поведение вместо того, чтобы быть failing security regression test.
5. Тестовая команда не может считаться полностью зелёной: network/socket тесты требуют окружения с разрешённым bind/connect.

## Исправления, внесённые после первичного аудита

В исходный проект внесены следующие изменения:

- `WsService.sendMessage` теперь возвращает `false`, если нет соединения или ключа получателя, и никогда не отправляет plaintext fallback.
- `AppState` учитывает результат отправки и сохраняет сообщение как `failed`, если зашифрованная отправка не состоялась.
- Убрана серверная совместимость с bearer token в query string: mobile API принимает только `Authorization` header.
- Пустой TLS fingerprint теперь отклоняется в API, WebSocket, discovery и pairing-клиентах Flutter.
- Session cookies получили флаг `Secure`.

Rust-проверка после изменений:

```text
CARGO_TARGET_DIR=/tmp/yandi-node-fix-check cargo check --offline
Finished successfully
```

`cargo fmt --all` в текущем окружении выполнить нельзя: sandbox монтирует часть репозитория read-only и форматтер получил `Read-only file system`. Flutter-анализ также не выполнен, потому что Flutter SDK отсутствует (`flutter: command not found`).

## Результат `cargo test --offline`

Команда была выполнена:

```text
CARGO_TARGET_DIR=/tmp/yandi-node-codex-review cargo test --offline
```

Сборка прошла. Итог тестов:

```text
355 passed; 36 failed; 0 ignored
```

Все 36 падений, которые удалось наблюдать, связаны с `Operation not permitted` / `PermissionDenied` на bind/connect сокетов в ограниченной среде выполнения. Например, первый сбой — `communication::chat::tests::the_queue_fails_only_day_old_messages_and_keeps_young_ones_for_resending` при bind порта `19402`. Это не доказывает логическую корректность сетевой части: для окончательной проверки эти тесты нужно повторить на обычной Linux-машине с разрешёнными loopback/network sockets.

## Рекомендованный порядок исправления

1. Убрать plaintext fallback: отсутствие подтверждённого peer key должно останавливать отправку; отдельно покрыть сообщения, receipts, edits и call signaling.
2. Ввести криптографическую привязку Ed25519 identity ↔ X25519 key ↔ peer ID и проверять её до помещения ключа в cache.
3. Запретить пустой fingerprint в release, удалить query-token compatibility path после миграции и добавить тесты на логирование/URL leakage.
4. Реализовать authenticated E2E envelope с AAD, sequence/epoch и replay protection; удалить или переименовать no-op `E2EEncryption`, пока он не станет настоящим.
5. Удалить `session_key_hex` из plaintext JSON либо зашифровать хранилище ключом ОС; добавить ротацию и invalidation.
6. Отключить RawIP по умолчанию и закрыть его peer authentication, quotas, idle timeout и rate limiting до повторного включения.
7. Исправить Rust media ECDH или исключить этот путь из production; отдельно документировать границы доверия WebRTC.
8. Добавить `Secure` cookie, безопасные bind defaults, pairing rate limiting и строгую авторизацию node introspection.
9. Добавить security regression tests: fail-closed encryption, key-binding forgery, replay, empty-pinning, token-in-URL rejection, RawIP unauthenticated access и cookie flags.
10. Повторить полный `cargo test --offline` вне sandbox; отдельно запустить сетевые integration tests в изолированном test namespace.

## Что нужно считать блокером релиза

До исправления пунктов #10, #11, #12, #15, #16 и #19 нельзя заявлять полноценную E2E-защиту и безопасный публичный режим ноды. До исправления #9 и #2 нельзя считать pairing/mobile transport устойчивыми к компрометации доверенной инфраструктуры или локального хранилища.
