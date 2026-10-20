# HANDOFF · Flutter desktop · P2 M1 · 2026-10-11

Этап M1 завершён в `/mnt/ssd480/wt/codex2`, ветка `codex/desktop-shell`, база `1938b42` (проект P2 поверх P1 `7389386`). На M1 работа остановлена. Исходники демона и веб-интерфейса не изменены, P3 не начиналась; main, `~/node` и исходники мобильного приложения не трогались, push не выполнялся. Исходный неизвестный `target-claude/` оставлен без изменений.

## Реализация

`desktop/yandi_desktop` — Linux GTK runner и Flutter Material UI без WebView. Зависимости исполнения: только SDK Flutter. Dart-клиент, модели состояний и формы не используют собственное постоянное хранилище.

- `GET /api/auth/state`: строго разбираются два bool; первичная настройка, восстановление привязки или обычный локальный вход. Неизвестный/повреждённый формат закрывает доступ с ошибкой протокола.
- Первичная настройка: четыре скрытых поля, проверка повторов, Unicode-длина login ≥8/master ≥12, разные пароли, предупреждение записать master. Один POST с защитой Enter; после подтверждения — ожидание запуска через GET, без повторной отправки POST.
- «Доступ к ноде»: объясняет авторизацию владельца по uid Unix-сокета. «Войти» читает vault и панель. Cookie/remembered/token не создаются и не сохраняются. Это локальный вход по контракту P1 и DESIGN, а не имитация браузерного password login.
- Восстановление пароля входа: существующий `/api/auth/recover`, поля master/code + новый пароль/repeat, `remember_me:false`. При `recovery_required` выводится существующая инструкция `yandi-keys recover`; перепривязка из Flutter не обещается и не выполняется.
- Панель: идентификаторы, IPv6/IP, роль/режим, NAT, порты, статус и active_proxies; список обнаруженных узлов с источником, ролью и портами; RX/TX, RTT, оценка peer RX и диагностические счётчики. Длинные значения переносятся, идентификаторы выделяются для копирования. Неизвестные значения — «—». Список узлов представлен карточками для работы и в узком окне.
- Узел/шлюз: кнопки вызывают существующие start/stop API; ответ 501 объясняет отсутствие реализации и управление системной службой. Статус не изменяется оптимистически. `/api/connect/*` честно обозначен как заглушка, фиктивное подключение не выполняется. Расширенные proxy/gateway экраны относятся к дальнейшему P2.
- Закрытая среда: создание (пароль/repeat и optional четыре слова, реальное подтверждение отказа от слов), unlock, lock, change_password, recover словами, auto_lock (секунды, 0…2592000). Минимум пароля vault — 6 Unicode-знаков по текущему Rust API; daemon сохраняет окончательную валидацию длины в байтах и слов. Отображаются незавершённый перенос и ошибка состояния.
- Vault poll 1 с; панель status 5 с, speed 2 с, nodes 10 с. Циклы не накладываются. На другой странице панель не опрашивается. При hidden/inactive/detached таймер останавливается, RPC отменяются и формы очищаются. На resumed перечитываются auth/vault; старая панель не воскресает из диска.
- При собственной lock сначала сбрасываются снимки и поля и инвалидируются старые ответы. Если блокировка не подтверждена, безопасный экран сохраняется до явной повторной команды; poll не открывает формы обратно. Remote lock очищает парольные формы при ближайшем успешном чтении состояния. Logout оболочки не меняет политику vault и явно объясняет необходимость «Запереть».

## Unix API и секреты

`HttpClient.connectionFactory` создаёт только `Socket.startConnect(InternetAddress(path, type: unix), 0)`. Проверяется тип конечного файла без следования symlink. Виртуальный HTTP origin — localhost, DNS/TCP connector не используется; `findProxy` — DIRECT. Нет redirect-follow, cookie jar, bearer или автоматического POST retry. Путь API проверяется, внешние URL не принимаются.

Приоритет socket: `--socket PATH` / `--socket=PATH` → `YANDI_UI_SOCKET` → `$XDG_DATA_HOME/yandi/run/yandi-ui.sock` (если переменная непустая) → `$HOME/.local/share/yandi/run/yandi-ui.sock`. CLI/env override требует абсолютный путь. HOME/XDG fallback соответствует Linux `util/data_dir` демона. Таймаут всего RPC — 15 с, принудительное закрытие IO; максимум 6 одновременных RPC. JSON-ответ ограничен 1 МиБ по Content-Length и фактическому числу байтов, включая chunked/decompressed stream; неверный JSON/тип/MIME и redirect отклоняются. HTTP-код хранится отдельно, legacy `status:error`, `ok:false`, `error` проверяются; поле `error` raw vault/status учитывается как состояние vault.

Тела, URL с пользовательскими данными, заголовки и секреты клиент не журналирует. Серверные error/message могут отражать пароль, поэтому UI показывает безопасное русское объяснение и HTTP-статус вместо произвольного server body. 401, 429, 501 различаются; 429 не вызывает автоматического retry и не угадывает паузу по русской строке. Это уменьшает подробность диагностической ошибки, сознательно сохраняя секреты.

Все данные приложения — в памяти. Нет записи истории, паролей, cookie, токенов, предпочтений и снимков ответа. GPU disk shader cache отключён через Mesa/NVIDIA параметры runner, GTK settings backend — memory. Поля имеют `obscureText`, выключенные autofill, autocorrect, suggestions и personalized IME learning. Контроллеры очищаются сразу при отправке и при уничтожении формы. Используются `Uint8List` UTF-8 копии; после сериализации/передачи они зануляются, request buffer зануляется после flush, response byte buffer — в finally. Карты и списки секретных строк очищаются после передачи владения запросу.

Ограничение: TextEditingController/engine, UTF-8 decode, JSON и системный IME создают неизменяемые String и внутренние копии; гарантированное физическое стирание всего heap невозможно. Не заявляется защита от swap/core dumps или от чтения памяти локальным отладчиком. Debug-сборка Flutter имеет штатный VM service и его служебный вывод; для распространения потребуется отдельная release-проверка. Ключи vault/identity в Dart не извлекаются. Клиент не проверяет SO_PEERCRED сервера (нет готового публичного Dart API); проверки владельца и каталогов выполняет P1 на стороне демона.

## Как собрать, запустить и проверить

Flutter `/home/iam/.local/flutter-sdk`, версия 3.38.9; Dart 3.10.8, Linux x64. Команды из каталога проекта:

```bash
cd /mnt/ssd480/wt/codex2/desktop/yandi_desktop
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter pub get --offline
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter analyze
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter test
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter build linux --debug --no-pub
./build/linux/x64/debug/bundle/yandi_desktop
# Для лабораторного сокета:
./build/linux/x64/debug/bundle/yandi_desktop --socket /absolute/test/data/run/yandi-ui.sock
```

Отдельный `pub get --offline` и `--no-pub` на build исключают сетевое разрешение пакетов. `build/` и `.dart_tool/` находятся внутри desktop проекта, игнорируются; GeneratedPluginRegistrant, generated_plugins.cmake, ephemeral engine files, .metadata и IDE-файлы не закоммичены. Linux runner/CMake templates закоммичены как необходимые исходники сборки. pubspec.lock не коммитится: runtime пакетов из pub нет, проверенная версия SDK указана выше.

Недостающие пакеты: **нет**. SDK/test-зависимости разрешились из имеющегося pub-кэша; сетевых установок и добавления пакетов не было.

## Выполненные проверки и результаты

1. `flutter pub get --offline` — успешно. `flutter analyze` — `No issues found`. `flutter build linux --debug --no-pub` — успешно, бинарь `build/linux/x64/debug/bundle/yandi_desktop`.
2. `flutter test` — **30 tests passed**. 19 транспортных/domain и 11 виджетных: настоящий временный Unix HttpServer, JSON POST и отсутствие cookie/auth headers, Content-Length/chunked oversize, timeout headers/stalled body, cancellation, отсутствие сокета, HTTP 200 business error/401/429/501/302, malformed/error JSON, byte wipe и выбор socket path, строгое auth/vault состояние; offline retry, настройка и single-flight Enter, recovery screen/contract, панель, скрытие/resume, формы всех vault операций, подтверждение без слов, remote lock wipe, поздний unlock после hide, неподтверждённая lock, validation и ширина 400 px.
3. Реальное GTK-окно запущено в Xvfb `:199`, размер 1280×720, в отдельном network namespace без внешней сети и с несуществующим socket. Процесс работал, X11 подтвердил окно `YANDI`; PNG просмотрен — экран «Нет связи с демоном», понятное объяснение и кнопка повтора. Последний запуск в выделенном HOME/XDG не создал там ни одного файла. Снимок и служебный вывод — локальные игнорируемые `desktop/yandi_desktop/build/offline-window.png` и `offline-window.log`. GTK ATK выдал `atk_socket_embed` warning в Xvfb; окно и отрисовка при этом работают.
4. `chaos/desktop_shell_m1.py` + `tool/daemon_probe.dart` — **PASS** на настоящем текущем демоне. Скрипт создаёт user/network/mount namespaces, tmpfs /tmp, loopback и временные HOME/XDG; использует тот же production UnixApi. Проверены auth/setup и handover, status/nodes/speed, создание vault, lock, 401 на неверный пароль, unlock, change password, auto_lock=1 с фактической блокировкой спустя 3 с (poll не считается активностью), вход новым паролем, восстановление словами, 501 node/gateway start, перезапуск с запертым vault и unlock восстановленным паролем. Временные данные удалены. Dart shim сообщает о root, поскольку uid 0 внутри user namespace; привилегий root на хосте нет.

Команда интеграции из корня репозитория:

```bash
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 python3 chaos/desktop_shell_m1.py /mnt/ssd480/yandi_codex_audit/target_20261011/debug/yandi
```

Использовался обновлённый текущими исходниками бинарь, собранный `cargo build --offline --bin yandi -j 2`, `nice -n 15`, без proxy, с `CARGO_TARGET_DIR=/mnt/ssd480/yandi_codex_audit/target_20261011`. Исходники Cargo и lockfile не изменились; существующие warnings демона оставлены. Первоначальный старый audit-бинарь не содержал P1 и не создал сокет; он заменён результатом сборки. Первая попытка user namespace выявила unmapped root uid у host /tmp: лаборатория теперь монтирует собственный tmpfs /tmp, удовлетворяющий P1 проверке доверенного sticky-каталога.

Для ручного GUI-прогона: используйте отдельный тестовый daemon HOME/XDG с приватным data_dir, запустите оболочку с `--socket`, пройдите настройку → «Войти» → «Закрытая среда», проверьте все формы и сворачивание; остановите/перезапустите только тестовый демон, повторите подключение. `daemon_probe.dart` нельзя направлять на сокет владельца: он меняет тестовые пароли/настройки. Для RPC-прогона предпочтителен готовый лабораторный скрипт выше.

## Локальные коммиты

- `b81468b` — `Implement Flutter Linux desktop M1 shell over Unix API`.
- `ff3e3b6` — `Test desktop M1 Unix transport, widgets and isolated daemon lifecycle`.
- Отдельный завершающий коммит `Document desktop P2 M1 verification and handoff` содержит этот отчёт и актуализированный статус DESIGN; его hash виден в `git log` и финальном сообщении.

Все `git add` выполнены поимённо; generated/build/.dart_tool и исходный target-claude не добавлялись.

## Эпизод проверки и риски

При ранней попытке получить справку существующий `yandi --help` неожиданно начал обычный запуск с реальным окружением пользователя и сетевой диагностикой: этот бинарь игнорирует --help. Процесс немедленно остановлен SIGINT, последующий запуск проводился только в лаборатории. Не было ручных операций с данными владельца; `~/node` и main не затрагивались. Отсутствие любых побочных файлов этого короткого запуска полностью не доказано; timestamp существующего `~/.local/share/yandi/ip_history.json` остался от 2026-10-09. Этот запуск не считается интеграционной проверкой.

Polling обнаруживает внешний lock при ближайшем успешном ответе; 1 с — период запуска, не гарантия при медленном демоне/длинном RPC. В скрытом окне polling отсутствует; при возврате vault перечитывается. При потере ответа POST пользователь должен проверить состояние вручную; повтор секретной команды автоматически не выполняется. Автоблокировкой владеет демон, её default и политика не изменены. В M1 UI/controller размещены вместе в app.dart; до расширения полного P2 разумно выделить controller/repositories по DESIGN. OS-specific peer trust, упаковка/release и дальнейшие экраны требуют отдельных этапов.

## Непроверенное на сдаче M1

- Полный ручной проход **GTK-форм с реальным демоном**: формы проверены widget-тестами, реальные RPC — отдельным Dart-прогоном; совместное сквозное GUI-тестирование не выполнено.
- Реальное восстановление identity/auth на машине с нарушенной привязкой: проверены state/форма/запрос и инструкция CLI, а не восстановление ключей в лаборатории.
- Физическое зануление всех String/engine/IME копий, swap/core dumps; полный syscall-аудит отсутствия записи при всех действиях (проверены исходники и чистый HOME только offline-окна).
- SO_PEERCRED сервера на стороне Dart, атакующая подмена socket, чужой uid в этом M1 прогоне (серверная защита относится к P1).
- Длительная нагрузка, точное время обнаружения remote lock при подвисшем RPC, daemon restart без наблюдаемого разрыва между poll, Wayland/другие оконные менеджеры, accessibility и системный IME вне Xvfb.
- Release-сборка, упаковка/распространение, другие ОС; чат, группы, передача файлов, звонки, уведомления и остальные этапы P2/P3 не реализованы и не проверялись.
