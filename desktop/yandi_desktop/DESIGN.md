# YANDI desktop — проект P2, 2026-10-11

Статус на завершение M2, срез 1: к M1 добавлены контакты по карточке, текстовый чат и настройки профиля/пароля входа/закрытой среды; проверки и ограничения — в `docs/HANDOFF_shell_m2_slice1.md`. Исторические проверки M1 — в `docs/HANDOFF_desktop_shell_P2M1_2026-10-11.md`. Ниже сохранён проект всего P2, включая будущие этапы, которые M1 не реализует. Основание: GUI_SPEC_2026-10-11.md и HANDOFF_desktop_shell_P1_2026-10-11.md в /mnt/ssd480/yandi_codex_audit. Полный контракт и русские строки закреплены в GUI_SPEC; документ ниже задаёт устройство будущей оболочки, а не новый wire-протокол. P1: локальный коммит 7389386. P3 не разрешена.

## Границы

Flutter desktop, собственные виджеты, без WebView/браузера. Все данные и долговременная криптография принадлежат демону. Не импортировать mobile/crypto, identity, group_store, file_vault, e2e_crypto: телефон является самостоятельным криптографическим клиентом, desktop — клиент существующего демона. Закрытая среда, peer trust и расшифровка файлов управляются теми же Rust API. Клиент не хранит DK, мастер-ключ, group/file key, мобильный bearer token либо plaintext history на диске. Временные SOCKS/TURN credentials, приглашение и QR существуют только в памяти до закрытия соответствующего экрана/lock.

Начальная поддержка — Linux, поскольку P1 использует SO_PEERCRED и /proc. Другие ОС требуют отдельного транспорта и проверки границы доступа. Никакого резервного подключения к TCP, запуска браузера или незаметного переключения на мобильный HTTPS API при ошибке socket.

Ни retention, ни VPN, TURN/nftables, решения №5/№14, политика инфраструктуры здесь не меняются. Новые ACK/NACK не вводятся. Существующие телефонные квитанции отображаются без расширения протокола.

## Будущая структура

```text
lib/
  main.dart                       # composition root, жизненный цикл окна
  app/desktop_app.dart            # навигация, тема, общие banners
  app/app_controller.dart         # startup + connection + vault coordination
  app/session_scope.dart          # epoch и закрытие чувствительных ресурсов
  domain/models/                  # DTO с явными nullable/enum unknown
  domain/repositories/            # интерфейсы, не зависящие от Flutter
  transport/unix_http_transport.dart
  transport/api_result.dart       # status, body, business error, safe reason
  transport/response_decoder.dart # неодинаковые legacy envelopes
  services/poll_scheduler.dart    # один владелец каждого poll
  services/file_transfer.dart     # read stream, chunk flow, progress, cancellation
  services/call_coordinator.dart  # все семейства звонков
  services/notifications.dart     # memory preferences, DND, locked redaction
  features/startup/               # setup/recover/запуск демона
  features/vault/                 # unlock/setup/recover/change/lock
  features/dashboard/
  features/chat/                  # contacts/profile/other devices также здесь
  features/groups/
  features/settings/
  features/gateways/
  features/relay/
  features/calls/
  widgets/                       # ConfirmDialog, StepUpDialog, state panels
  l10n/                          # русские строки GUI_SPEC, единый каталог
assets/                          # только проверенные изображения/звук
linux/                           # создаётся позднее инструментами Flutter
test/transport/, test/domain/, test/controllers/
test/widgets/
integration_test/               # только после команды P3
```

Чистый Dart слой: DTO, state machines, transport abstraction, decoder, scheduler, draft isolation и transfers; без imports Flutter/GTK/plugins. UI обращается к repository интерфейсам. Тесты подменяют transport и часы; отдельные настоящие UnixStream-тесты используют временный socket. Выбор стороннего state management/DI/plugin и версии SDK откладывается до проверки имеющегося SDK и desktop поддержки. Для базовой модели достаточно неизменяемых snapshot и stream/controller; не выбирать библиотеку ради схемы.

## Unix API-клиент

HttpClient.connectionFactory создаёт соединение только с `<data_dir>/run/yandi-ui.sock` (Dart Unix socket). HttpClient использует виртуальный origin `http://localhost`, чтобы сформировать HTTP request и Host; origin не разрешается DNS и не отправляется в сеть. Factory не вызывает стандартный TCP connector. Порт в URI служит HTTP-парсеру, не реальному соединению. Proxy для этого клиента отключён; окружение HTTP_PROXY не влияет на Unix transport. Нужны ConnectionTask и корректная отмена connect: точная сигнатура и поддержка Unix address сверяются с установленным Dart SDK при реализации, сейчас код с непроверенной сигнатурой не предлагается.

Data dir разрешается по тем же Linux правилам, что Rust util/data_dir: XDG_DATA_HOME/yandi или HOME/.local/share/yandi. Для лаборатории допустим явный путь socket; путь не содержит секретов, но запрещён произвольный remote URL. Ключи/пароли не извлекаются из конфигов владельца. Перед соединением клиент проверяет тип socket и доступность, сообщает ошибку прав; аутентификационное решение остаётся за daemon peer credentials. Дополнительная проверка server peer credentials, если Dart/plugin её предоставляет, — предмет реализации, не заявленная гарантия этого проекта.

Пул клиента ограничить шестью соединениями: четыре коротких API, одна передача, один WS. Серверный бюджет 32 не следует исчерпывать при нескольких окнах. На фоне upload зарезервировать control slot для lock/status. Сервер отключает HTTP keep-alive, поэтому клиент должен корректно открывать новые Unix streams. Никаких cookie jar, remember_me, Authorization bearer для обычных UI API. WebSocket строится через тот же custom HttpClient, не через отдельный TCP/WebSocket connector; проверка фактического upgrade — обязательный gate будущей реализации. WS heartbeat каждые 20 s, отмена на lock, server IO idle 60 s.

Разрешённые API-пути строятся Uri pathSegments/queryParameters с percent encoding каждого ID/имени. Не конкатенировать пользовательские строки в raw URL. Для upload multipart — byte stream с предсказуемой длиной и boundary, content-type точный. Range скачивания поддерживает 200/206 и 416; имя файла не определяет доверенный MIME. 201 MiB — ceiling транспортного запроса, не обещание такого размера для всех endpoints.

Каждый запрос несёт memory scope epoch и resource identity (peer/group/request revision). При ответе сверяются все три; старый ответ игнорируется и освобождается. Abort закрывает запрос/stream, а не просто скрывает indicator. Deadline клиента меньше daemon 300 s, но различается для короткого API и transfer; длительные передачи явно показывают timeout. Не журналировать тела, headers с секретами, пароль, phrase, invite, SDP, ICE, TURN/SOCKS credentials или полный URL с пользовательскими query.

Result decoder хранит HTTP status отдельно от результата. HTTP 200 может содержать `{status:"error",message}`, `{ok:false,error}` либо `{error}`. Success envelopes различаются: `{ok:true}`, `{status:"ok"}`, `{message}`, raw array `/api/groups/devices`, `{profile:...}`, nullable incoming/turn. HTML redirect по API не является success: 3xx не follow, typed ProtocolMismatch. Socket transport errors = disconnected, не «неверный пароль». 401/429 при secret operation остаются отдельными ошибками. 400/404 показывают контекст и безопасную причину; 501 означает отсутствие реализации; неизвестные поля/enum не приводят к падению.

## Модель состояния и блокировка

Три независимые оси:

| Ось | Состояния | Эффект |
|---|---|---|
| Соединение | connecting, online, disconnected, retrying, protocolError | banner и disabled сетевых действий; не подмена vault состояния |
| Начальная настройка | checking, setupRequired, startingDaemon, ready, recoveryRequired | GET /api/auth/state определяет экран |
| Закрытая среда | checking, notConfigured, locked, unlocking, open, migrationPending, broken, lockPending | gate чувствительных экранов |

Startup: connection → auth/state → setup/recovery по флагам → vault/status → gate → основной shell. Обычного password login/cookie с remembered на сокете нет: uid уже разрешает API. Заголовок экрана «Доступ к ноде» объясняет локальную модель; реальные парольные формы используются для setup, recovery, StepUp и vault. Не имитировать browser login ради соответствия картинке.

Setup четыре поля, прежняя Unicode-проверка login≥8/master≥12, разные пароли и repeats; одна in-flight команда, Enter использует тот же guard. После успеха tiny listener исчезает, состояние «Ключи созданы. Нода запускается…», повторные соединения к socket и auth/state до 120 s с отменой. Не повторять POST setup автоматически после потери ответа. Recovery переиспользует существующий endpoint; перепривязка показывает фактическую инструкцию yandi-keys recover, а не обещает рабочую форму. Нужен отдельный лабораторный прогон recovery, прежде чем публиковать этот экран.

Vault: notConfigured → setup password/repeat и optional четыре recovery слова, предупреждение при отказе от слов; locked → unlock или recover; open → change password, timeout, lock. Ответы 401 и 429 не вызывают retry секретов; показывают ошибку/доступную задержку. migration_pending и error не скрываются зелёным статусом. Auto lock secs 0 либо до 30 days валидируется по существующему API, новый default не выбирается. Polling сам по себе не считается пользовательской активностью.

При команде lock сначала увеличить local epoch, закрыть чувствительные routes/WS/calls/transfers, очистить memory stores и поля, затем вызвать daemon lock. Ошибка RPC оставляет безопасный экран «Состояние блокировки не подтверждено»; не восстанавливать старый snapshot. При обнаруженном remote lock, изменении daemon generation, restart или потере доверенного соединения — тот же wipe. После reconnect заново прочитать auth/vault и данные, не воскресить drafts.

Удалить drafts всех peers, messages/groups/contacts/profile, file bytes/preview, invitation/QR, временные credentials, timers, notifications payloads, selection/edit, media tracks. Uint8List с чувствительными bytes заполнить нулями перед освобождением; TextEditingController очистить и dispose. Immutable Dart String и копии в GC/engine/plugin нельзя гарантированно занулить: минимизировать lifetime и ссылки, не заявлять физическое стирание всей памяти. Долговременных ключей в Dart всё равно нет.

Daemon не посылает отдельного vault push в текущем API. При внешнем auto-lock отображённый snapshot может оставаться до ближайшего status poll; предусмотреть vault poll раз в 1 s при открытом scope и проверку после resume/перед sensitive action. Команда lock самого клиента очищает сразу. Обещание мгновенного обнаружения внешней блокировки без серверного события технически неверно; интеграционный тест должен измерить эту границу. При закрытии/сворачивании окна не устанавливать новую политику auto-lock владельца: очистка при exit обязательна, существующий timeout задаёт daemon.

## Экраны, виджеты и порядок переноса

| Очередь | Экраны/виджеты | Контракт и функции, которые сохраняются |
|---|---|---|
| 1 | ConnectionGate, SetupPage, RecoveryPage, VaultGate, StepUpDialog | startup flags; четыре setup поля, reveal/сила без бага score=5; unlock/recover/change/timeout, migration/error; реальные confirm и пароль StepUp |
| 2 | NavigationRail, StatusCards, NodeTable, SpeedPanel | identity/CID/IPv6/NAT/role/ports/copy; discovered nodes mDNS/P2P; status 5 s, nodes 10 s, speed 2 s; node/gateway 501 и connect stub честно обозначены |
| 3 | ContactList, ConversationView, Composer, PeerDetails, ProfileEditor, TransferTile, AttachmentPreview, OtherDevices | CRUD/search/export, online, tunnel actions/status, drafts по peer, edit/select/delete-local/clear, emoji, chunks/progress/cancel, profile/avatar, hide/block/add устройства, приглашение группы |
| 4 | MyGroups, GroupCatalog, GroupAdmin, GroupFeed, GroupMembers, JoinRequests, ModerationLog, OwnershipTransferDialog | create/join/request/leave/forget; has_key/problem/damaged/gone; deny rules, файлы, invite-code/device, accept/never, роли/moderation, transfer с подтверждением и отсчётом 3 s, StepUp delete metadata |
| 5 | SettingsForms, ClosedEnvironmentPanel, TrustedPeersPanel, UpstreamPoolPanel, PhoneDevicesPanel, PairingDialog | ports/IP/log/restart; card import/export/remove, exit mode/warning; bulk proxy/partial errors/caps/strict/health/test; revoke device отличать от hide; QR expiry |
| 6 | GatewaysPage, ProxySessionCard, RelayPage | saved gateways CRUD/country; фактические HTTP/SOCKS outputs, auth credentials только в памяти; relay stats/sessions/connect, точные errors/stubs |
| 7 | IncomingCallDialog, CallView, VideoView, AudioControls | общий coordinator, node voice/video WS + phone signaling, accept/reject/end/mute/camera/level/duration, ring/DND/notification, ICE и resource cleanup |

Legacy contacts превращается в полноценную часть чата/отдельную navigation destination, а не теряется из-за отсутствующего script в HTML. На широком окне чат три панели; при узком — list/detail с сохранением peer selection. Длинные IDs/filenames переносятся или копируются целиком; цвета online/own/other/selected дополняются текстом/иконкой. Навигация не сбрасывает vault gate. Form drafts отделены от live server snapshot, polling не трогает фокусный input.

### Чат, вложения и память

GET history возвращает последние 100, pagination в API нет; не рисовать бесконечную историю. GET истории телефона выполняет mark-read: опрашивается только выбранный открытый разговор, не все контакты в фоне. Initial snapshot не порождает уведомлений об архиве. Scroll следует за новыми сообщениями только если пользователь уже внизу; edit/attachment diff учитывается, а не сравнивается только текст.

Contact mutations, delete/clear и edit ждут реальный decoder success. `for_everyone:false` сохраняет нынешний UI смысл; не добавить незапрошенное удаление у адресата. Tunnel requested и active разные состояния. Online разных источников не обещает доступный chat. Export contacts и сохранение файла — явное пользовательское действие с выбранным destination, не скрытый cache.

Node chunks 700 B, phone chunks 1 MiB, existing node limit 200 MiB и phone 1024 MiB. Последовательный upload читает источник небольшими блоками, учитывает file_id первого ответа, после complete вызывает chat send. Не повторять chunk/send автоматически при неоднозначной потере ответа; возможный duplicate показывать как неопределённый результат. Zero-byte attachment должен быть либо явно отклонён по текущему API, либо реализован только после отдельного исправления daemon; не выдумывать total_chunks=0 success. Cancel закрывает запрос и предотвращает последующий send, но не обещает очистить daemon staging без API отмены.

Group raw upload до 200 MiB выполняется stream; encryption и hash checks в daemon. После P1 UI body не содержит `k`; decoder не требует его. Файлы группы скачиваются через `/api/groups/{gid}/file/{seq}`. Узловые file_ref проходят status/content; limited retry ожидания содержимого возможен без повторного upload, максимум пять попыток по спецификации, с epoch guard.

Предпросмотр raster/audio/video только из bytes/stream в памяти, без HTML/SVG renderer и auto-opening system browser. Media/file picker плагины должны быть проверены на plaintext temp files: plugin, принимающий только filename и записывающий скрытый cache, не подходит. Для больших медиа нужен ограниченный stream/range источник; без него честное «предпросмотр недоступен, сохранить файл» до завершения реализации, а не скрытая запись. Явное сохранение пользователем разрешено; старый plaintext staging демона — отдельный серверный долг P0 и не разрешение на shell cache. Профиль и QR также не используют disk image cache.

### Группы

Cursor after предназначен для новых записей; текущий API не передаёт tombstones в incremental ответе. Чтобы удалённые/запрещённые сообщения не оставались, после moderation/rule change перезагрузить full after=0 и периодически сверять выбранную ленту полностью. Запросы membership/info и feed имеют разные loading/error states. Group admin узла и participant role не одно и то же: StepUp удаления hosted group не заменять кнопкой удаления локального следа. group devices response raw array, mobile admin devices prefix16; не смешивать id/alias/full peer. Invite-code wrapper не отдаёт expires_ms — expiry не выдумывать. Spent invites не пишутся в localStorage-аналог; память scope либо состояние daemon.

Ownership handover сохраняет подтверждение/ожидание из спецификации, повторная отправка не создаётся таймером. has_key=false, problem, damaged, gone требуют разных русских сообщений; нельзя считать любую такую группу пустой.

### Настройки, шлюзы и relay

Шесть портов и IP/log сохраняются по фактическому API, requires_restart явно показан. Не добавлять неизвестные status поля uptime/bytes как нули. Speed `_mbps` в старом API фактически MiB/s: нормализовать подпись и units, Path0 loss не назвать потерей доставленных сообщений.

Trusted card bound key остаётся проверкой daemon; remove подтверждается. Exit mode all показывает существующее warning и выбирается только пользователем. Upstream password отправляется при добавлении, GET показывает has_password, не выдумывает сохранённый пароль. Partial bulk success показывается с added и errors. HTTPS scheme старого парсера не означает TLS до proxy. Статус cooldown/clients/caps/health сохраняется, focused drafts не перетираются. Lock не обещает сетевой kill-switch: старый daemon может перейти на direct после сброса locked pool.

Шлюзы: metadata proxy stop не гарантирует остановку listener в старом коде; отличать подтверждённые API данные от обещания «порт закрыт». Адрес/порт/username из ответа, не hardcoded 8080/1080. Disconnect/connect stubs не превращаются в оптимистическую зелёную связь. Relay source filter не должен молча скрывать нужный peer; показывать source и фактический статус, idle/bytes/duration как разные метрики.

Phone revoke закрывает sockets и отзывает token, а hide/remove contact этого не делают. Pairing GET мутирует code и инвалидирует прошлый QR: не включать в auto-retry/poller. Диалог генерирует по явному действию, отображает qr_text локально нативным QR renderer (не SVG WebView), таймер 300 s очищает payload и требует нового действия. Хост вводится пользователем/подставляется по daemon, код/QR не копируются в debug log.

## Polling и ошибки

Один scheduler на app, subscribe/unsubscribe по видимости и открытому scope. Для каждого resource один запрос одновременно, следующий запуск после завершения; нет перекрывающихся setInterval-подобных запросов. GET retry exponential backoff 1/2/4/8/15 s + jitter, только для безопасных status/list чтений. POST/PATCH/DELETE, pairing GET и read-side-effect history не повторяются автоматически. При 429 задержка берётся из фактического ответа, если есть, иначе разрешается ручное повторение; не угадывать по regex русской строки.

| Resource | Базовая частота | Условие |
|---|---|---|
| vault/status | 1 s | открытый scope, плюс resume/перед действием |
| status / speed / nodes | 5 / 2 / 10 s | панель видима |
| contacts / selected history | 5 / 5 s | список/разговор открыт; mark-read только выбранный |
| other devices / groups list | 10 / 15 s | видимый chat/list |
| selected group feed/info | 4 s, full reconciliation 15 s | видимая выбранная группа; info после feed, forms отдельно |
| proxies/devices / trusted peers | 5 / 15 s | соответствующий settings panel |
| gateways / relay status,sessions | 10 / 5 s | страницы видимы |
| node incoming voice/video | 3 s | unlocked call coordinator |
| phone call events | 1 s | unlocked и звонки включены, в том числе фон окна |

Последние три строки относятся к будущему этапу звонков; сейчас их нет в коде shell. В phone event stream первый last задаёт baseline; старые события не звонят. Недоступность >6 s в текущем daemon влияет на возможность телефона звонить компьютеру; не выключать этот poll просто при переходе на settings. В background UI тяжёлые списки можно замедлять, call events и lock monitoring оставляются пока scope открыт.

Ошибки отображаются в экране и общем connection banner; toast не единственный носитель. Неизвестный daemon reason выводится как plain Text с контекстом, без innerHTML и разбора «просрочено» по локализованной строке. После reconnect refresh всех видимых snapshots; forms не восстанавливают secrets. Данные marked stale не выглядят текущими. 501, empty, loading, forbidden/locked, damaged и disconnected — разные виджеты.

## Звонки — последний этап

Единственный CallCoordinator для node voice, node video, phone. Состояния idle/incomingRinging/outgoingRinging/connecting/active/ending/ended/error, active family/peer/cid. Контроль сверяет peer/call_id; второй invite получает busy по существующему протоколу. Node signal schema и phone `\\u0001yandi-call:` строго по GUI_SPEC, не унифицировать wire types собственным форматом. Node flags сейчас не выбирают video: нужен правильный route family.

WS node через custom Unix HttpClient; phone signal/events/log/turn — API того же socket. E4/Replay/закрытые ключи остаются в daemon, DTLS-SRTP медиастек располагается в shell. TURN временный username/credential в памяти, не новое решение владельца об инфраструктуре. ICE очередь до remote description, ring 45 s и disconnect grace 10 s для phone сохраняются; deadline connecting добавить на уровне UX с cleanup, значение подтвердить испытаниями, не менять wire. Поздний camera/mic acquire после ended закрывает полученные tracks.

DND применяется ко всем звукам, включая phone, notifications скрывают текст при lock. Системные уведомления/аудио Linux требуют выбора desktop plugins и проверки закрытия/кэша, поэтому реализация не объявлена готовой. Sound/язык можно хранить только в памяти первой версии, чтобы не вводить неразрешённый disk store; eventual nonsecret preferences отдельно. Русский — основной язык; English можно добавить позднее, не теряя существующие RU строки.

## Русские строки и клавиатура

GUI_SPEC приложение B — каталог исходных русских строк; переносить смысл всех подтверждений/ошибок, не баги и ложные обещания. Общие новые состояния: «Нет связи с нодой», «Подключение к ноде…», «Требуется первичная настройка», «Закрытая среда заперта», «Разблокировать», «Идёт перенос данных», «Не удалось открыть закрытую среду», «Состояние блокировки не подтверждено». Основная навигация: «Панель», «Чат», «Группы», «Настройки», «Шлюзы», «Relay». Backend English reason получает русский контекст без потери диагностической причины и без secrets.

В личном чате Enter отправляет только через единый guard, Shift+Enter вводит новую строку; повторное Enter при in-flight не дублирует команду. В группе сохраняются Ctrl/Meta+Enter для отправки и Enter для новой строки; Escape закрывает emoji/dialog и снимает selection без опасного действия; Tab/focus/disabled работают. Delete/clear/remove/revoke/leave/transfer требуют понятного подтверждения с конкретным объектом, StepUp там, где требует daemon. Clipboard по явной команде, секреты автоматически не копируются; гарантировать очистку общесистемного clipboard нельзя. Никакого confirm=always true и исполнения сообщения как HTML.

## План проверок полного P2 (за пределами выполненных проверок M1)

После появления SDK чистые Dart/controller tests через flutter test без GTK: Unix connection factory и отмена; отсутствие TCP fallback/proxy/cookies; decoding array/null/E200/401/429/501; stale epoch/resource responses; lock wipe и поздний unlock/media response; single-flight polls/backoff; неидемпотентный retry запрещён; QR GET не повторяется; drafts peer isolation; initial notification baseline; file chunk progress/cancel/no double-send; group key не требуется; DND всех calls. Для Unix transport fake server и настоящие Unix temp sockets с fixtures ответов Rust tests, не live ключи владельца.

Widget tests: startup/vault gates, формы/Enter, modal confirm/StepUp, empty/stale/disconnected/damaged, keyboard/resize, long filenames, focused settings drafts, progress и notifications redaction. Нужен render/visual review каждой страницы, а не только совпадение названий кнопок.

Linux desktop build после установки владельцем toolchain: ninja, clang, libgtk-3-dev и нужные Flutter/plugin dependencies. Установка sudo здесь не выполняется. Сборка GUI и media plugin не заменяет integrated daemon/socket проверки. По отдельной команде P3: lab TCP vs Unix full-state parity, настоящий чужой uid, 201 MiB/phone transfer, websocket upgrade/heartbeat/call families, daemon restart/setup/recovery, auto-lock timing/wipe, no plaintext shell temp/cache and logs. После прохождения и отдельного разрешения на P3 можно удалять UI/HTML-only маршруты и документацию.

M1 проверен отдельной Linux debug-сборкой, flutter test, GTK-окном без демона и реальным daemon/socket RPC-прогоном; подробности в handoff M1. Полный P2 не реализован: WebRTC, уведомления, transfers/previews, чат/группы и расширенные настройки остаются будущими этапами. P3 не начиналась.
