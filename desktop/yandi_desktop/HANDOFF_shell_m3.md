# HANDOFF: оболочка M3 — шлюзы, Relay, звонки, 2026-10-12

Рабочая копия `/mnt/ssd480/wt/codex3`, ветка `codex/shell-m3`, основа `5db8ab3`. Выполнен TASK_shell_m3_2026-10-12.md. Веб-UI сохранён; решение P3 не принималось. Ключ закрытой среды остаётся у демона; оболочка не хранит и не расшифровывает историю/сигналы E4.

## Шаг A

Коммит `dfcf164` — `desktop: add gateway and relay management (shell M3 step A)`.

- В навигации «Шлюзы» и «Relay». Опрос каждые 5 секунд, ручное обновление; ответы после уничтожения экрана отбрасываются. Перед мутациями — повторная проверка vault; единая защита от повторной отправки, без POST retries.
- Шлюзы: список, имя/Short ID/страна, соединение и метаданные прокси от демона, неизвестная задержка «—». Добавить/изменить/удалить; настоящее подтверждение удаления. Валидируются Short ID 16 hex, непустое имя и страны RU/US/NL/DE/FR/GB. После CRUD перечитывается список: ответ старого обработчика сам по себе не доказывает сохранение.
- HTTP и SOCKS5: раздельные статусы, порты/адреса из ответа демона, start/stop. При активном прокси второй start недоступен. Нет оптимистического «остановлено»: status сообщает наличие записей; stop явно объясняет, что закрытие слушающего сокета API не подтверждает. Это существующий серверный долг; stop только удаляет метаданные. Настоящие connect/disconnect узла — заглушки, UI не выдаёт их за работающие действия.
- SOCKS-пароль отображается только из ответа start, в памяти экрана. При следующей операции/уходе/блокировке ссылки удаляются; собственных файлов/логов/clipboard с паролем нет.
- Relay: режим сервера, счётчики байт/пакетов/сессий, сессии с направлением/статусом/простоем; start/stop с перечитыванием режима; обновление peer picker и connect по P2P Short ID. Успех connect означает отправку запроса, не гарантию публичной доступности. Сохранён фильтр source=p2p; известный mDNS dedup демона может скрыть подходящего peer.

## Шаг B

Коммит с названием `desktop: add unified call signaling and coordinator (shell M3 step B)` включает этот отчёт, код звонков, регрессионные тесты и точечное исправление Unix WebSocket демона.

- Единый `CallCoordinator` для голоса/видео и узлов/телефонов: idle → incoming/outgoing ringing → connecting → active → ending → ended/error. Приём/отклонение/завершение, busy при другом звонке, mute и камера через медиа-адаптер. Active возникает только от события connected медиа-адаптера; сам accept его не создаёт.
- Ringing deadline 45 секунд; выбранный connecting deadline 30 секунд; disconnect grace 10 секунд, восстановление connected отменяет grace; длительность раз в секунду, terminal экран закрывается через 2 секунды. Таймеры и очередь сигналов принадлежат epoch; старые control/ответы/ошибки не оживляют завершённый вызов.
- Проверяются peer и call_id/cid, семейство медиа; ICE ожидает remote description, буферы ограничены 128 сигналами. Для старого node SDP/ICE без call_id разрешён только соответствующий тип на WS, привязанном к peer текущего вызова. Старый сервер не проверяет call_id в VoiceData; это остаётся ограничением wire-протокола.
- Node API voice/video start/incoming/accept/reject/end; WebSocket через тот же Unix-сокет, без TCP fallback/cookies/tokens; ping каждые 20 секунд поддерживает Unix WS при отсутствии новых сигналов. Nullable incoming JSON допустим только на двух точных incoming endpoints. Query разрешён только `/api/mobile/call/events?after=N`; остальные query по-прежнему отвергаются.
- Phone events: первый ответ устанавливает baseline last без воспроизведения архива; seq сортируется и продвигается, проверяются v=1, cid 8–64, video bool, timestamp ±90 секунд и полный ID телефона. Формируется совместимый префикс `\u0001yandi-call:`; signal/turn/log идут через демона. В историю отправляется только фиксированная краткая запись 📞 о завершении/ошибке, best effort без повторов. SDP, ICE и TURN credentials не логируются и не записываются оболочкой на диск.
- При уходе с экрана, hide, logout, lock, disconnect уничтожается координатор: tracks/PC должен закрывать адаптер, WS/подписки/таймеры/очереди закрываются, ссылки очищаются. Cleanup peer notification — best effort. Завершение доступно и во время ожидания медиа; remote hangup немедленно прерывает pending acquisition. Поздно открывшийся адаптер дополнительно закрывается.
- Входящие опрашиваются только на открытом экране «Звонки», каждые 1 секунду для обоих семейств узлов и телефона. Навигация закрывает вызов. Глобального фонового звонкового overlay пока нет.

### Почему медиа подставное

В локальном pub-кэше есть `flutter_webrtc-1.6.2+hotfix.4`, но отсутствуют `third_party/libwebrtc/lib/libwebrtc.so` и архив нативной библиотеки. Его CMake скачивал бы её из сети. Пакет не добавлялся, сеть/pub download не использовались. По разрешённому заданием fallback реализован `CallMedia` и `UnavailableMedia`: при принятии вызова честная ошибка и cleanup/hangup, никогда фиктивный active.

Для настоящего WebRTC нужны: согласованное получение/проверка нативной библиотеки и зависимостей; реализация CallMedia для RTC PC, SDP/ICE, stream tracks, mute/camera; преобразование node SDP-object и phone SDP-string/cand; TURN для телефона и согласованные STUN параметры узлов. Реализовать local/remote preview, уровень микрофона, camera 640×480 с audio-only/recvonly fallback, аудиовывод/выбор устройств. Проверить реальные voice/video ПК↔ПК и ПК↔телефон, NAT/TURN, отсутствие устройств, потерю сети и поздний getUserMedia. Подставной адаптер/тесты не доказывают работу медиа.

### Обнаруженный блокер Unix WS и исправление

На исходном демоне реальный upgrade возвращал HTTP 101 с `Connection: close`, поэтому Dart отвергал handshake. Причина: Hyper `keep_alive(false)` заменял Connection даже у 101. В `src/web/ui_socket.rs` это снято; middleware добавляет `Connection: close` всем обычным RPC-ответам, сохраняя настоящий 101 Upgrade. Проверены закрытие RPC и маскированный WS echo; ограничения uid, путей, тела, времени и бюджета соединений сохранены. Для звонков требуется демон с этим исправлением, одного обновления Flutter-оболочки недостаточно.

## Проверки

Команды Cargo/Flutter/Dart и лаборатория — без HTTP_PROXY/HTTPS_PROXY/ALL_PROXY и lowercase вариантов. Cargo offline, Flutter --no-pub, новых зависимостей нет.

- `flutter test --no-pub --reporter expanded`: 70 PASS, включая M1/M2 и новые экраны, SOCKS password wipe, ошибки сохранения/stop, настоящие подтверждения, preflight lock, узкое окно, поздние ответы, координатор, таймеры, ICE ordering, remote hangup при pending media, phone baseline/cid/peer, Unix null/query/WS/cancel.
- `flutter analyze --no-pub`: No issues found.
- `flutter build linux --debug --no-pub`: PASS; bundle в desktop/yandi_desktop/build, не коммитится.
- `cargo test --offline --lib web::ui_socket::tests -- --test-threads=1`: 7 PASS (включая новый upgrade/echo/RPC close).
- `cargo build --offline --bin yandi`: PASS; имеются прежние предупреждения компилятора. Сборка и независимая копия старого build cache находятся только в рабочей копии target; каталог аудита с прежним бинарём не изменён.
- `python3 chaos/desktop_shell_m1.py /mnt/ssd480/wt/codex3/target/debug/yandi`: PASS. Сохранены M1 vault/restart и M2 чат/группы; добавлены gateway CRUD, HTTP/SOCKS start/stop metadata, Relay server/connect/sessions; voice/video start/incoming/accept/reject/end и тестовый SDP через настоящий Unix WS двух узлов. Медиа не открывается. Тестовые узлы, HOME/XDG/CWD — только /tmp в отдельном user/network/mount namespace; процессы и данные удаляются. Боевой узел и systemd не затрагивались.
- `python3 -m py_compile chaos/desktop_shell_m1.py`, `git diff --check`: PASS. Ручного GTK и телефонного E2E не было. Полный Rust suite не запускался: изменён только Unix HTTP/WS транспорт, проверен его набор и реальная лаборатория.

Логи: `/tmp/codex3-m3-tests.log`, `/tmp/codex3-m3-calls-tests.log`, `/tmp/codex3-m3-rust-tests.log`, `/tmp/codex3-m3-daemon-build.log`, `/tmp/codex3-m3-build.log`, `/tmp/codex3-m3-lab-final.log`. Ранние прогоны с исходным демоном выявили описанный WS блокер; итоговый PASS получен с исправленным бинарём.

## Остаток паритета по GUI_INVENTORY — основа P3

Сверены разделы inventory app.js, chat, contacts, groups, index, login/setup, phone-call, relay, settings, speed, voice/video; дополнены исправлениями GUI_SPEC 13–14.

| Семейство веб-UI | Что ещё отсутствует/частично в оболочке |
|---|---|
| app.js / общая оболочка | RU/EN, звуки/DND/рингтоны, OS notifications с корректным baseline и privacy, логотип/упаковка media и лицензии, системная интеграция. Неработающие legacy buttons/pollers переносить не нужно. |
| index / speed | Явные copy-ID действия, часть дополнительных диагностических счётчиков; управление службой node/gateway остаётся 501 демона. Connect/disconnect stub не считать работающей функцией. HTTP/SOCKS доступны на отдельном экране шлюзов. |
| contacts | Экспорт contacts.json, карточка собственного узла/copy, отзыв доверия отдельно от удаления контакта, tunnel start/stop/status, файлы. |
| chat | Редактирование/удаление/очистка истории, файлы/chunks/status/content/watch и реальные progress/preview/download, аватары (view/upload/delete), устройства other-devices и add/remove/hide/block, интерактивные приглашения из сообщения, emoji UI, скролл/уведомления непрочитанного. Текст/контакты/профиль и текущая история уже есть. |
| groups | Загрузка/скачивание/preview файлов и медиа, emoji UI, picker телефонов `/api/groups/devices` вместо ручного полного ID; дополнительные presentation/pickers. Основные роли/правила/приглашения/transfer/StepUp реализованы M2; без аудита файлов не обещать полный паритет. |
| gateways / relay | Основные сценарии перенесены M3. Отсутствует действительное подтверждение остановки proxy listener — серверный долг; country flags и человекочитаемые форматтеры bytes/idle пока простые текстовые. |
| settings | Рабочие порты/bind/log и restart banner, pairing QR/TTL, список/отзыв paired devices, own card/trust-remove, exit policy, upstream proxy pool CRUD/settings/test/health/cap, маршруты auto/rules/hops/страна и personal gateway. Vault/профиль/восстановление пароля входа уже есть. |
| voice/video/phone-call | Нативный WebRTC и перечисленные выше media-функции; фоновые входящие/overlay вне экрана «Звонки», звуки/notifications и интеграция кнопок звонка с контактами/чатом, телефонный E2E. Координатор и совместимые сигнальные пути готовы; реальное медиа не готово. |
| login/setup | Вход по uid — намеренная локальная модель, browser cookies/remembered/login/logout не нужны для Unix-shell. Reveal/индикатор силы — необязательный визуальный остаток. Rebind реально CLI-only; не переносить ложное обещание старой формы. |

Вывод для владельца P3: удалять веб-UI сейчас нельзя — остаются устройства/сопряжение, файлы, расширенная сеть и настоящее медиа. Само решение об удалении остаётся за владельцем. Никаких изменений веб-UI/боевого узла не выполнено.

Неизменяемые Dart Strings физически гарантированно не зануляются: wipe удаляет ссылки/контроллеры и ограничивает время жизни. Внешняя блокировка определяется общим vault poll (~1 секунда + RPC задержка). SOCKS stop metadata-only и server CRUD swallowing disk errors не исправлялись скрытно. Коммиты только по явно названным путям; GeneratedPluginRegistrant.java/CODEX_REVIEW_*/bug_r14_repeat_* не включены.
