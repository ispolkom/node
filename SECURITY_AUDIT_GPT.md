# YANDI Node / Mobile — security audit

Автор: GPT
Дата аудита: 2026-10-09
Область: `/home/iam/node`, Rust-нода и Flutter/Android-клиент

## Краткий вывод

Общая криптографическая база выглядит серьёзно: используются X25519, Ed25519, AES-GCM, счётчики пакетов, replay-window и certificate pinning. Однако в текущем состоянии есть несколько важных проблем в восстановлении сессий, мобильном транспорте и хранении ключей.

## Найденные проблемы

### 1. Resume-токен не жёстко связан с identity отправителя — высокий риск

В `src/netlayer/transport.rs`, функция `handle_resume_packet`, токен ищется по `session_id`, а MAC проверяется по секрету токена и адресу. В отдельном WebSocket fast-path уже есть проверка `resume_node_matches`, но этот общий handler должен иметь такую же проверку; сохранённая привязка `node_id_hex` там не проверяется перед восстановлением `session_key`.

Риск: если resume-токен/секрет украден, его можно предъявить с другой identity и попытаться восстановить сессию для неправильного Node ID.

Рекомендации:

1. Включить canonical sender identity в данные, подписываемые MAC.
2. Проверять `embedded_node_id` против identity владельца токена до `restore_session`.
3. При несовпадении не отправлять ACK и не менять состояние сессии.
4. Добавить тест: токен A, sender B → отказ; состояние encryption store не меняется.

Файлы: `src/netlayer/transport.rs:5471-5528` и WebSocket fast-path около `src/netlayer/transport.rs:980-1045`.

### 2. `session_key_hex` хранится открытым текстом — высокий риск

`SessionToken` содержит AES session key в открытом виде и сохраняет его в `paired_clients.json`.

Риск: чтение файла, резервной копии или snapshot диска позволяет восстановить канал, подделывать трафик и расшифровывать данные до истечения токена.

Рекомендации:

1. Не хранить traffic/session key на диске.
2. Хранить только encrypted resume-secret.
3. После resume выполнять новое эфемерное X25519-рукопожатие.
4. Устанавливать новый traffic key только после подтверждения обеих сторон.
5. Мигрировать старые токены безопасно, не логируя секреты.

Файл: `src/netlayer/pairing.rs:75-86`.

### 3. Мобильный клиент допускает незашифрованный HTTP/WS — критично при удалённом подключении

`TrustedNode` выбирает `ws://` и `http://`, если fingerprint пустой:

`mobile/yandi_mobile/lib/models/trusted_node.dart:32-39`.

Android также разрешает cleartext:

`mobile/yandi_mobile/android/app/src/main/AndroidManifest.xml:30`.

Риск: bearer-токен, сообщения и управляющие запросы могут передаваться без TLS и перехватываться в локальной сети.

Рекомендации:

1. Для удалённой ноды отсутствие fingerprint считать ошибкой конфигурации.
2. Запретить `ws://`/`http://`, кроме явно разрешённого loopback-режима разработки.
3. Установить `android:usesCleartextTraffic="false"`.
4. Добавить regression test: remote node без pin не подключается.

### 4. Bearer-токен передаётся в URL WebSocket — высокий риск утечки

Клиент подключается через:

`/mobile/ws?token=<token>`

Файл: `mobile/yandi_mobile/lib/services/ws_service.dart:126`.

URL может попасть в proxy/access-логи, трассировки, crash reports и диагностические дампы.

Рекомендации:

1. Передавать токен через `Authorization: Bearer ...` в WebSocket handshake.
2. Серверу запретить авторизацию через query string после миграционного периода.
3. Никогда не логировать полный URL запроса.
4. После миграции ротировать ранее использованные токены.

### 5. Session cookie не имеет флага `Secure` — высокий риск при HTTP UI

`make_session_cookie` задаёт `HttpOnly` и `SameSite`, но не задаёт `Secure`.

Если UI доступен по HTTP, браузер может отправить сессионную cookie открытым текстом.

Рекомендации:

1. Добавить `Secure` для production cookie.
2. Запретить логин и state-changing API через HTTP.
3. Для локальной разработки использовать отдельный режим без внешнего bind.

Файл: `src/web/auth.rs:624-640`.

### 6. Нет обязательного E2E между устройствами людей — архитектурное ограничение

Документация указывает, что телефон → собеседник не имеет полностью независимого E2E слоя. В ряде сценариев содержимое защищено только TLS до доверенной ноды.

Риск: владелец ноды, скомпрометированная нода или relay могут видеть обычный текст.

Рекомендации:

1. Сделать E2E обязательным для сообщений между телефонами.
2. Проверять ключ получателя и fingerprint/key identity.
3. Добавить защиту от downgrade к plaintext.
4. Хранить на ноде только ciphertext и метаданные, необходимые для доставки.

Файл: `docs/CLIENT_WIRE.md:43-45`.

### 7. Сетевые интерфейсы по умолчанию слушают `0.0.0.0` — повышенная поверхность атаки

По умолчанию WebSocket и серверный bind используют все интерфейсы:

`src/core/config.rs:66,89`.

Риск: интерфейсы могут стать доступными из LAN/WAN при ошибке firewall или port forwarding.

Рекомендации:

1. По умолчанию использовать `127.0.0.1`.
2. Внешний bind включать только явной настройкой.
3. При внешнем bind требовать TLS, pinning, rate limit и firewall confirmation.
4. Явно разделить localhost admin API и public transport API.

### 8. Bearer-токен мобильной ноды хранится в обычной SQLite-базе — высокий риск

Комментарий в `StorageService` говорит, что токены хранятся в `flutter_secure_storage`, но таблица `nodes` содержит обычную колонку `token`, а `saveNode` и `updateNodeMetrics` записывают туда токен.

Риск: дамп приложения, root-доступ, незашифрованная резервная копия или forensic-доступ к `yandi.db` раскрывает доступ к ноде.

Рекомендации:

1. Удалить `token` из SQLite-схемы.
2. Хранить токен только в Android Keystore через `flutter_secure_storage`.
3. В SQLite оставить только `token_key_id`/идентификатор секрета.
4. Мигрировать старые базы и удалить старую колонку/значения.
5. После миграции ротировать ранее сохранённые токены.

Файлы: `mobile/yandi_mobile/lib/services/storage_service.dart:42-56,118-133`.

### 9. TLS pinning отключается при пустом fingerprint — критично

В Dart-клиенте пустой fingerprint возвращает обычный `http.Client`, а WebSocket-клиент принимает любой сертификат. В Android `YandiService` также сразу принимает сертификат при пустом fingerprint.

Риск: MITM-атака возможна именно в аварийном/неполном сценарии конфигурации — там, где приложение должно остановиться.

Рекомендации:

1. Для production всегда требовать непустой fingerprint.
2. При отсутствии pin возвращать ошибку подключения.
3. Оставить unpinned режим только под debug-флагом, недоступным в release APK.
4. Добавить тесты для Dart, Kotlin и VPN-пути: пустой fingerprint → соединение отвергнуто.

Файлы: `mobile/yandi_mobile/lib/services/api_service.dart:243-248`, `mobile/yandi_mobile/lib/services/ws_service.dart:336-344`, `mobile/yandi_mobile/android/app/src/main/kotlin/com/yandi/yandi_mobile/YandiService.kt:419-426`.

### 10. Отправка сообщений откатывается в plaintext при отсутствии ключа — критично

В `WsService.sendMessage` при недоступном публичном ключе получателя выполняется:

```dart
payload = utf8.encode(text);
```

Это нарушает гарантию E2E: ошибка получения ключа превращается в отправку открытого текста.

Рекомендации:

1. Удалить plaintext fallback.
2. Если ключа нет — не отправлять сообщение и показать ошибку/статус ожидания ключа.
3. Принимать plaintext только для явно маркированного legacy-чата и только по явному policy-флагу.
4. Добавить тест: недоступен ключ → в wire не появляется исходный текст.

Файл: `mobile/yandi_mobile/lib/services/ws_service.dart:255-273`.

### 11. X25519-ключи телефонов не аутентифицированы — MITM-риск

Нода принимает `ed25519_pub` и `x25519_pub` через `/mobile/pubkeys`, но сервер не проверяет подпись связывающую X25519-ключ с Ed25519 identity. Клиент получает X25519-ключ через `/mobile/pubkey/:peer` и кэширует его.

Риск: скомпрометированная или вредоносная нода может заменить публичный X25519-ключ и провести подмену ключа между телефонами. Сам факт TLS до ноды этого не устраняет.

Рекомендации:

1. Подписывать canonical payload: `peer_id || x25519_pub || version || expiry` ключом Ed25519.
2. Проверять, что `peer_id == SHA-256(ed25519_pub)`.
3. Кэшировать и сравнивать key fingerprint; смену ключа требовать подтверждать пользователем или отдельным rotation-протоколом.
4. Не считать данные от `/mobile/pubkey` доказательством identity без подписи.

Файлы: `src/mobile_api.rs:480-507`, `mobile/yandi_mobile/lib/services/api_service.dart:104-133`.

### 12. E2E blob не имеет связанного контекста и replay-защиты — средний риск

`E2ECrypto` использует X25519 + AES-GCM, но в AES-GCM не передаётся AAD с sender, recipient, message id, типом сообщения и версией протокола. Формат содержит ephemeral key и nonce, но не содержит отдельного sequence/message counter.

Риск: аутентичный ciphertext можно повторно доставить или перенести в другой контекст, если верхний слой не выполнит строгую дедупликацию.

Рекомендации:

1. Ввести canonical AAD с версиями, sender ID, recipient ID, message ID и типом payload.
2. Добавить monotonic message ID/sequence и окно replay на устройстве получателя.
3. Проверять, что sender из payload совпадает с ожидаемым peer.
4. Отдельно определить политику повторной доставки для offline inbox.

Файл: `mobile/yandi_mobile/lib/crypto/e2e_crypto.dart:16-72`.

### 13. Pairing endpoint имеет ограниченную, но сетевую поверхность перебора

Pairing code живёт пять минут и допускает пять ошибок, что лучше полного отсутствия лимита, но код является коротким и endpoint доступен до аутентификации.

Рекомендации:

1. Добавить rate limit по IP/соединению и экспоненциальную задержку.
2. При ошибках не раскрывать, истёк код или неверен.
3. Ограничить pairing только локальным интерфейсом/первоначально доверенной сетью либо требовать подтверждение на UI ноды.
4. После успешного pairing сразу ротировать одноразовый код и инвалидировать старые попытки.

Файл: `src/mobile_api.rs:281-319`.

### 14. Автоматические тесты не являются полностью зелёными

В sandbox-запуске `cargo test --all-targets` результат: **355 passed, 36 failed**. Большинство сетевых падений вызвано `Operation not permitted` при bind/connect сокетов в ограниченной среде, но один тест очереди чата упал отдельно и требует проверки вне sandbox.

До релиза нужно разделить тесты на:

1. чистые unit/crypto tests;
2. локальные integration tests;
3. сетевые tests, требующие разрешённого namespace;
4. mobile instrumentation tests.

CI должен явно показывать, какие security-тесты реально выполнялись, а какие были пропущены из-за ограничений окружения.

### 15. Media encryption не выполняет X25519 ECDH — критично

В `src/media/session/stream.rs::establish_shared_secret` создаётся `EphemeralSecret`, но он не сохраняется и не участвует в вычислении ключа. Вместо ECDH ключ вычисляется как `SHA-256(remote_public)`.

Это означает, что любой наблюдатель, знающий public key, может вычислить тот же AES-ключ. Это не является shared secret и не обеспечивает конфиденциальность.

Дополнительная проблема: локальный ratchet меняет encryption key после 100 пакетов, но decrypt path не имеет симметричного подтверждённого перехода по счётчику. После ротации стороны могут потерять синхронизацию.

Рекомендации:

1. Хранить ephemeral private key до завершения handshake.
2. Вычислять `X25519(local_ephemeral_secret, remote_public)`.
3. Использовать HKDF с transcript/session ID и раздельными send/receive keys.
4. Аутентифицировать handshake Ed25519-подписью или уже доверенным transport channel.
5. Сделать ratchet явным протоколом с номером эпохи и подтверждением смены ключа.
6. До исправления отключить media path в production.

Файл: `src/media/session/stream.rs:115-130,134-175`.

### 16. Legacy RawIP tunnel выглядит как неаутентифицированный открытый туннель — критично при включении

`src/netlayer/rawip_tunnel.rs` слушает `0.0.0.0:<port>`, принимает соединения по TCP и разбирает RawIP-пакеты до проверки identity, TLS, MAC или session token. Команда `rawip-exit` реально запускает этот listener на TCP-порту `10001`. В коде также нет очевидного ограничения числа соединений и peer-level byte quota.

Риск: если этот компонент доступен из сети, любой клиент может отправлять IP-пакеты через ноду или создать большое число соединений. Это превращает endpoint в open relay/DoS surface.

Рекомендации:

1. Не запускать RawIP listener по умолчанию.
2. Удалить legacy path либо закрыть его за тем же authenticated encrypted transport.
3. Проверять identity и capability peer до первого IP-пакета.
4. Добавить per-peer connection limit, global byte budget, idle timeout и packet rate limit.
5. Отклонять source spoofing и проверять допустимость destination.
6. Привязать listener к localhost или явно настроенному interface.

Файлы: `src/netlayer/rawip_tunnel.rs:122-145`, `src/netlayer/cli.rs:654-680`.

### 17. TUN routing вызывает системные команды из сформированных строк — средний риск сопровождения

`setup_routing` и `cleanup` формируют команды `ip ...` строковой конкатенацией, затем разбивают их через `split_whitespace` и передают в `Command`.

Прямой shell injection здесь не происходит, потому что shell не запускается, но значения `device_name` и `ipv6_addr` могут превратить команду в другую последовательность аргументов, если они попадут из недоверенной конфигурации.

Рекомендации:

1. Передавать аргументы напрямую массивами, без конкатенации строк.
2. Валидировать interface name строгим allowlist-паттерном.
3. Валидировать IPv6 через `IpAddr`, а не строковыми проверками.
4. Не принимать routing parameters от удалённого peer.

Файл: `src/netlayer/tun_device.rs:610-655`.

### 18. Telemetry/introspection запускает внешние утилиты

Node introspection вызывает `ping6`, `curl`, `dig`, `ip`, `ifconfig`, `netstat` и другие системные команды. Сейчас аргументы в найденных местах в основном константные, поэтому это не подтверждённая injection-уязвимость.

Риски сопровождения: зависимость от PATH, подмена бинарников в нестандартном окружении, неожиданные внешние сетевые запросы и блокирование worker thread.

Рекомендации:

1. Использовать абсолютные пути или Rust networking APIs.
2. Ограничить introspection отдельным capability/permission.
3. Всегда применять timeout и лимит вывода.
4. Не запускать внешние команды с данными, полученными от peers или web API.

Файлы: `src/netlayer/node_introspection.rs`, `src/netlayer/interface_detector.rs`, `src/netlayer/external_ip.rs`.

### 19. `communication::E2EEncryption` — фактически no-op — высокий риск

`src/communication/encryption.rs` прямо возвращает plaintext без шифрования. Чат полагается на transport encryption, а отдельного E2E между конечными пользователями нет.

Риск: relay, промежуточный узел или владелец транспортной ноды может видеть сообщения. Наличие класса с названием `E2EEncryption` создаёт ложное ощущение защиты.

Рекомендации:

1. Переименовать текущую обёртку в `TransportOnlyEncryption` до реализации настоящего E2E.
2. Не считать transport encryption E2E-гарантией.
3. Ввести authenticated E2E protocol с key continuity, AAD, replay protection и rotation.
4. Добавить тест, который проверяет, что wire payload не равен plaintext.

Файл: `src/communication/encryption.rs:1-35`.

## Дополнительный проход по ноде

Проверены Rust-исходники ноды, включая:

- Web/API routing и auth middleware;
- UDP/TCP/WebSocket/TLS transports;
- pairing и RESUME;
- crypto/session/rekey/replay;
- P2P relay, proxy, SOCKS5, exit policy и RawIP;
- chat, file transfer, media и storage;
- TUN/routing и вызовы системных команд;
- path/file handling и лимиты входных данных;
- unsafe/FFI места и тестовые сетевые пути;
- `Cargo.toml`/`Cargo.lock` и версии зависимостей.

Положительные результаты прохода:

- file IDs и многие имена файлов проходят отдельную валидацию;
- web routes находятся под auth middleware, public routes выделены отдельно;
- host guard против DNS rebinding присутствует;
- upload body limit и лимиты файлов присутствуют в основных web-путях;
- transport crypto имеет отдельные тесты на replay, AAD, counters и key rotation;
- identity/private-file path содержит проверки небезопасных прав и symlink-сценариев.

Оставшиеся зоны, которые требуют отдельного runtime/pentest этапа:

1. Проверка всех API-методов на корректность auth/authorization не только по router, но и по бизнес-логике.
2. Проверка гонок при параллельных file upload/delete/resume операциях.
3. Полный fuzzing packet parsers и bincode/JSON decoders с лимитами памяти.
4. Runtime-проверка UDP/TCP/WS rate limits и exhaustion-сценариев.
5. Реальная проверка firewall, systemd sandboxing, capabilities и открытых портов.
6. Dependency advisory scan.

`cargo deny check` в текущем окружении не выполнил advisory scan: Cargo advisory database недоступна для записи из-за read-only пути `/home/iam/.cargo/advisory-dbs/db.lock`. Это не означает, что зависимости безопасны; проверку нужно выполнить в обычном CI/рабочей среде.

## Что уже выглядит хорошо

- Certificate fingerprint pinning реализован в мобильном клиенте.
- Основной transport использует AEAD и AAD.
- Есть защита от replay и счётчики пакетов.
- Слабые X25519 public keys отклоняются.
- Private files используют отдельный helper с ограниченными правами.
- Есть тесты на повреждённые кадры, replay, изменённые заголовки и неверные ключи.

## Рекомендуемый порядок исправлений

1. Запретить plaintext HTTP/WS для production.
2. Убрать token из WebSocket URL.
3. Добавить `Secure` к cookie и запретить login через HTTP.
4. Исправить identity binding в RESUME.
5. Убрать открытый `session_key_hex` из persistent storage.
6. Добавить обязательное E2E для mobile-to-mobile сообщений.
7. Перевести default bind с `0.0.0.0` на `127.0.0.1`.

## Результат тестового запуска

Команда `CARGO_TARGET_DIR=/tmp/yandi-node-audit-target cargo test --all-targets` собрала проект.

- 355 тестов прошли.
- 36 тестов не прошли.
- Большинство падений сетевых тестов вызвано ограничением среды аудита: `Operation not permitted` при bind/connect сокетов.
- Один тест очереди чата упал отдельно и требует самостоятельной проверки.

Это означает, что сетевые тесты нельзя считать полностью подтверждёнными в текущем sandbox-окружении.

## Ограничения аудита

Аудит был статическим и локальным. Не выполнялись атаки на внешние адреса, не перехватывался реальный пользовательский трафик и не изменялись исходники. Следующий этап — написать regression tests для перечисленных пунктов и исправлять их по одному с проверкой миграции мобильного клиента.
