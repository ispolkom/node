# Промт для Codex/Claude — ревью исправлений YANDI

Скопируй этот промт целиком в Codex (или другой агент) в корне репозитория `/home/iam/node`.

---

Ты — строгий ревьюер безопасности и корректности. Репозиторий: Rust-узел `/home/iam/node` + Flutter-приложение `mobile/yandi_mobile`. Есть внешний аудит `SECURITY_AUDIT_GPT.md` (19 находок). Часть находок УЖЕ исправлена — проверь их по коду, НЕ верь на слово ни аудиту, ни коммит-сообщениям.

Задача: проверить реализацию и записать результат в НОВЫЙ файл `CODEX_REVIEW_RESULT.md` (не меняя остальной код). Для каждого пункта укажи: ПОДТВЕРЖДЕНО/НЕ ПОДТВЕРЖДЕНО/ЧАСТИЧНО, файл:строка, и почему.

Проверь конкретно (недавно заявлены как исправленные):
1. #4 — токен сокета в заголовке `Authorization`, а не в URL. Смотри `mobile/yandi_mobile/lib/services/ws_service.dart` (`_tryConnect`) и серверную проверку `src/mobile_api.rs` (`auth`, маршрут `/mobile/ws`).
2. #3/#9 — нет cleartext и нет отката на http/ws. Смотри `android/app/src/main/AndroidManifest.xml` (`usesCleartextTraffic`), `lib/models/trusted_node.dart` (`httpUrl`/`wsUrl`). Убедись, что НИГДЕ в приложении не остаётся http:// или ws:// к узлу.
3. #8 — bearer-токен узла в `flutter_secure_storage`, не в открытой SQLite. Смотри `lib/services/storage_service.dart` (`loadNodes`/`saveNode`/`updateNodeMetrics`/`deleteNode`, ключ `node_token_`). Проверь корректность миграции старых баз и что токен больше НЕ пишется в колонку `nodes.token`.
4. Статусы доставки / квитанции / правка / чёрный список — не ломают E2E и не отправляют служебные метки открытым текстом не тем адресатам. Смотри `lib/models/receipt.dart` (`MsgChannel`), `lib/services/app_state.dart` (`_classifyIncoming`, `_applyReceipt`, `_applyEdit`, `isBlocked`), `lib/services/ws_service.dart`.

Затем проверь НЕ исправленные находки — подтверди, что они всё ещё актуальны, и оцени реальную эксплуатируемость в текущей архитектуре (узел владельца доверенный, телефон↔телефон E2E, телефон→обычный контакт — транспортное шифрование):
- #11 (x25519-ключи телефонов не аутентифицированы), #10 (plaintext-fallback в `WsService.sendMessage`), #12 (нет AAD/счётчика в `lib/crypto/e2e_crypto.dart`), #15 (`src/media/session/stream.rs` — SHA-256 вместо ECDH; используется ли реально мобильными звонками — они на flutter_webrtc), #16 (`src/netlayer/rawip_tunnel.rs` — запускается ли по умолчанию), #2 (`session_key_hex` на диске), #19 (`src/communication/encryption.rs` — заглушка).

Дополнительно: прогони `cargo test --offline` (сетевые тесты в песочнице могут падать из-за `Operation not permitted` — это ок, отметь отдельно); отметь любые НОВЫЕ регрессии, внесённые недавними правками.

В `CODEX_REVIEW_RESULT.md`: раздел «Подтверждено исправлено», «Всё ещё актуально (с оценкой риска)», «Новые проблемы/регрессии», «Рекомендованный порядок». Будь конкретным (файл:строка), без общих слов.
