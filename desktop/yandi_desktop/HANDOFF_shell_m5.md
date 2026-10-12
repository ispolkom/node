# HANDOFF shell M5 — устройства, групповые файлы, настройки, RU/EN

Дата: 2026-10-12. Рабочая копия `/mnt/ssd480/wt/codex3`, ветка `codex/shell-m5`. Основа: TASK_shell_m5_2026-10-12.md, HANDOFF_shell_m4.md, GUI_SPEC §6–8, 13–14 и GUI_INVENTORY. Веб-UI сохранён. `src/` не менялся, ключ закрытой среды и криптография остаются у демона. Новых пакетов и сетевых загрузок нет.

## Коммиты в порядке задания

1. `489c008` — `desktop: add phone pairing TTL and device access controls (shell M5 step 1)`.
2. `566cd4c` — `desktop: add group file transfer previews and phone picker (shell M5 step 2)`.
3. `58b21b3` — `desktop: add network settings exit policy sealed proxy pool and about (shell M5 step 3)`.
4. `desktop: complete shell localization and background unread alerts (shell M5 step 4)` — RU/EN, оповещения, окончательные проверки и этот HANDOFF.

## Выполнено

- **Устройства:** список `/api/mobile/devices`, online/connections/last_seen, отзыв по полному peer ID с подтверждением. QR и непрозрачный код `qr_text` только из `/api/mobile/pairing`; TTL считается от начала запроса, максимум 300 секунд, без автопродления. QR рисуется из проверенных прямоугольников существующего Rust renderer; общего SVG/HTML-движка и загрузок нет. Секрет и QR удаляются при истечении срока, скрытии кода, уходе/блокировке/отзыве. Параллельная генерация исключена общим single flight.
- **Чат/телефоны:** «Другие устройства» с new/hidden/blocked, add/hide/unhide/block/unblock через phone-action; для сохранённого телефона отдельные remove/hide/block. Действия подтверждаются, после успеха перечитываются списки; выбранная история, черновики и вложения закрываются при изменении доступа. Hide/remove/block не выдаются за отзыв pairing: настоящий отзыв — в настройках.
- **Групповые файлы/медиа:** локальный chooser, отправка до 200 MiB через существующий raw `/api/groups/{gid}/upload?name=…`, ограниченное последовательное чтение через file handle без shell staging. Прогресс означает байты, переданные HTTP-клиентом демону; подтверждение загрузки — ответ `seq`, не квитанция доставки. Пустые файлы отклоняются. Проверяются membership/key/mute/deny; демон повторно проверяет права и сам шифрует. При ошибке выбранный файл остаётся. Уход прекращает чтение; retry/resume/cancel API не выдумываются.
- **Приём групповых файлов:** отображаются только filename/size и seq; поля k/h/f/c из body не сохраняются в модели ленты. Предпросмотр растров в памяти (8 MiB), явное скачивание и «Сохранить и открыть» image/audio/video системным приложением через `/api/groups/{gid}/file/{seq}`. Расшифровка и SHA256 у демона. Существующие файлы не перезаписываются, неполная загрузка удаляется. Ключ preview меняется вместе с group/seq; память изображения очищается при закрытии. Inline audio/video player не добавлен: сценарий как в M4.
- **Телефон в группу:** picker из `/api/groups/devices` (реальный ответ ARRAY нормализуется в UnixApi только для этого пути). Ручного полного ID больше нет; выбранный ID проверяется как 64 lowerhex.
- **Настройки:** шесть рабочих портов, IP bind, уровень журнала через GET/PUT `/api/settings`; баннер перезапуска только по `requires_restart`. Node_mode/gateway legacy заглушки не перенесены. Политика выхода off/trusted/all через `/api/exit/settings`.
- **Прокси:** создание SOCKS5/HTTP CONNECT, подтверждённое удаление, включение записи, правка индивидуального cap, общий enabled/strict/cap; безопасный просмотр state/client count/has_password. Вся запись пула, в том числе без пароля, разрешена оболочкой только при configured+unlocked vault без migration/error; дополнительно выполняется штатный общий vault preflight. Пароль из отдельной формы сразу удаляется из контроллера, карта запроса очищается после отправки; сетевой буфер зануляется существующим UnixApi. Пароль из GET не запрашивается. Редактирование host/auth/password существующей записи не поддержано сервером и не подменено опасным удалением/созданием.
- **О приложении:** отдельная навигационная страница, версия оболочки из текущего pubspec (0.1.0), границы Unix API и хранения ключей. Версия демона не выдумывается.
- **RU/EN:** локальный каталог всех статических форм, подтверждений, ошибок, состояний и динамических подписей; поле ввода/валидация/роли/медиа/счётчики также переведены. Names, сообщения, карточки, ID и filenames сохраняются verbatim. Нет сетевого переводчика. Язык и предпочтения остаются сессионными. Незнакомые технические значения демона показываются как пришли.
- **Фоновые оповещения:** отдельный opt-in, общие sound/OS/DND; до 32 доверенных узловых контактов и 8 собственных групп, последовательный poll 5 s, включая скрытую оболочку при действующем локальном входе. Запоминаются только ограниченные наборы msg_id/seq и timestamp, до 100 записей на чат, без текста/имён/ключей. Первый снимок, свои/архивные/повторные сообщения не оповещают. Счётчик показывает новые чаты, замеченные в текущей сессии, а не серверный total unread. Открытие чата/группы снимает его локальный индикатор. Группы без ключа/повреждённые/с изменившимся владельцем пропускаются. Статус vault проверяется до и после чтения; lock/logout/disconnect/hide сбрасывают baseline. После hide первый новый снимок вновь становится baseline. ОС получает только фиксированную фразу без содержимого. Телефонные history не опрашиваются в фоне: GET отправляет read receipts.

## Проверки

Flutter/Dart/лаборатория — с удалёнными HTTP_PROXY/HTTPS_PROXY/ALL_PROXY и lowercase вариантами; Flutter `--no-pub`. Cargo не запускался: использован существующий `target/debug/yandi`. Ничего не скачивалось.

- `flutter test --no-pub --reporter expanded`: **106 PASS**. Новые виджет-тесты устройств/TTL/отзыва/late response, групповых вложений/picker/rejection, портов/restart, sealed guard/proxy editor/About, EN форм/диалогов/ошибок и звонков. Проверены права всех групповых ролей, whitelist без file keys, notification baseline/privacy/DND/lock/reset и групповой unread. Все прежние M1–M4 тесты проходят.
- `flutter analyze --no-pub`: **No issues found**.
- `flutter build linux --debug --no-pub`: **PASS** с существующим WebRTC/GTK cache.
- `chaos/desktop_shell_m1.py`: расширен и выполнен на двух изолированных временных узлах. Сохраняет M1–M4; добавлены devices/invalid phone-action, group devices ARRAY, raw upload Unicode filename и точное сравнение 1537 байт download, settings PUT/requires_restart, exit policy roundtrip, sealed proxy create/read/patch/delete. Fixture проверяет отсутствие upstream_proxy.json, наличие upstream_proxy.enc и отсутствие пароля в ciphertext и ответе GET.
- `python3 -m py_compile chaos/desktop_shell_m1.py`, `git diff --check`: **PASS**.

Логи: `/tmp/codex3-m5-final-tests2.log`, `/tmp/codex3-m5-analyze-final.log`, `/tmp/codex3-m5-final-build.log`, `/tmp/codex3-m5-final-lab2.log`. Один промежуточный повтор лаборатории прервался на Unix-связи после vault setup; добавлен вывод хвоста fixture daemon log при исключении, без изменения Rust/боевого узла. Причина первого обрыва не установлена.

Ручная проверка сканирования QR живым телефоном, доставки ОС toast/звука, GTK chooser/system viewer, живых звонков и сетевого tunnel handshake не выполнена. GNotification bridge не доказывает фактический показ toast. Виджет-тесты проверяют очистку ссылок, но не физическое зануление immutable Dart Strings/codec/OS buffers. Явный download/open/export/clipboard остаётся действием пользователя.

## Остаток паритета перед P3 / что мешает удалить веб-UI

| Семейство | Остаток |
|---|---|
| Общая оболочка | Desktop packaging/integration, лицензии media; отдельные рингтоны/online sound. Фоновый unread телефонов требует API без mark-read; для большого числа чатов нужны серверные summary/events вместо bounded polling. |
| Панель | Явные copy-ID действия, дополнительные диагностические счётчики и представление. Node/gateway service control 501 и connect/disconnect stub — серверный долг. |
| Контакты/чат | Интерактивные group invitation cards из сообщения, emoji UI, scroll/unread presentation, отдельный file modal, inline audio/video. Живые phone file/tunnel E2E. |
| Группы | Emoji и дополнительные presentation/pickers; inline playback. Streaming/chunk upload/resume/cancel отсутствуют в серверном API. |
| Шлюзы/relay | Actual listener stop (metadata-only в демоне), флаги стран/форматирование bytes/idle, live network E2E. |
| Настройки | Routes auto/rules/hops/country и personal gateway; proxy test/расширенные health panels; атомарная правка адреса/логина/пароля существующего proxy entry требует нового API. QR адрес по умолчанию из демона; ручной host override не добавлен. |
| Voice/video | Live node↔node/phone, NAT/TURN/devices/permissions/audio output E2E, фоновые входящие/overlay, рингтоны и кнопки звонка из чата/контактов. |
| Login/setup | Намеренный локальный uid доступ сохранён; reveal/strength — визуальный остаток, rebind CLI-only. |

## Серверный долг и границы

- `/api/upstream-proxies/{id}` patch умеет только enabled/max_clients. Endpoint изменения host/kind/auth/user/password отсутствует. Прямое создание в демоне без configured vault по-прежнему допускает plaintext upstream_proxy.json: оболочка этот путь блокирует, серверный запрет не добавлялся.
- Нет endpoint unread/summary для телефона без read receipts. Фоновые уведомления телефона нельзя корректно получить через `/api/chat/history`.
- Group messages API по-прежнему присылает секретный file body k. Оболочка сразу проецирует display whitelist, но убрать ключ из самого HTTP JSON может только демон.
- Raw group upload не имеет chunk/resume/cancel и полного ACK/progress API. При отмене shell закрывает соединение; уже принятые демоном действия не откатываются. Большие групповые снимки ограничены существующим JSON limit UnixApi.
- Pairing endpoint не имеет явного cancel/revoke кода: очистка UI скрывает секрет, daemon TTL остаётся настоящим сроком действия. Нет безопасного способа обещать аннулирование кода при уходе.
- Durability contacts/gateways, daemon file plaintext staging, audit file_id binding/quota/collisions, avatar physical removal/distribution, proxy listener stop и остальные долги M4 не исправлялись скрытно.

**Веб-UI удалять нельзя:** ещё нет перечисленных сетевых/маршрутных сценариев и доказанного живого паритета. P3 остаётся решением владельца. Build/, GeneratedPluginRegistrant.java, CODEX_REVIEW_* и временные fixtures в коммиты не включены.
