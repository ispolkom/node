# P1 — Unix-сокет оболочки YANDI, 2026-10-11

Рабочая копия: `/mnt/ssd480/wt/codex2`, ветка `codex/desktop-shell`, исходная база `80f6bc599a4e6c9afba5d18ea6bbd7111b74c821`. P0 завершён отдельными GUI_SPEC и REVIEW_signed_release; его выдержки исходников относятся к базе до P1. Исходники другой рабочей копии, main, ~/node не менялись, push не выполнялся. Артефакты Cargo помещены в общий target, прямо указанный владельцем.

## Реализация и модель доступа

`src/web/ui_socket.rs` — Linux Unix HTTP/1.1 transport. В обычном режиме и при первичной настройке слушатель находится в `<data_dir>/run/yandi-ui.sock`. Router клонируется с общими State и обработчиками; копий API нет. По сокету доступны только `/api/*`: HTML, статика и исторические `/pair/*` не выдаются. Для штатного телефонного сопряжения интерфейс уже использует `/api/mobile/pairing`; исторический протокол `/pair/issue` не превращён в новый клиентский протокол.

`UnixStream::peer_cred()` на Linux получает SO_PEERCRED. До HTTP-парсера и чтения запроса uid сравнивается с effective uid демона. Несовпадение/ошибка — закрытие. После проверки в Request добавляется закрытая capability LocalOwner; создать её вне transport-модуля нельзя, HTTP-заголовком подделать нельзя. Capability разрешает миновать только браузерную cookie-проверку. StepUp обработчиков и ограничения `/api/vault/*` остаются прежними. При setup разрешение по сокету также заменяет проверку TCP Host с точным портом; TCP-проверка не изменена.

Peer credentials достаточны для обычных административных API: эти процессы уже принадлежат владельцу данных демона. Это граница по uid, не по исполняемому файлу; любой процесс того же uid имеет доступ. Пароль входа нужен при первичной настройке, восстановлении/перепривязке и существующих StepUp. Мастер-пароль закрытой среды передаётся демону только для предусмотренных операций настройки/разблокировки/смены. Долговременные ключи закрытой среды клиенту не выдаются. Нативный «выход» не реализуется вызовом cookie logout: нужен lock закрытой среды и очистка памяти клиента. Модель не добавляет пароль к каждому локальному запросу.

Добавлен общий `GET /api/auth/state` в setup и основной Router: `{setup_required: bool, recovery_required: bool}`. Только несекретные флаги, без session/token/ключей. TCP endpoint также публичен под прежним Host guard. Это позволяет оболочке определить нужный экран без загрузки HTML.

При P0 найден лишний экспорт ключа вложения групп: `group_client::messages` теперь убирает поле `k` из UI-копии body. Внутренний журнал не меняется; расшифровка скачиваемого файла остаётся в демоне. Поля имени, размера, hash, идентификаторов и обычный текст сохранены. Это изменение общего API совместимо с существующей вебмордой, которая ключ для скачивания не использует.

## Права, жизненный цикл, ограничения

- data_dir и run принадлежат effective uid, закрыты для группы/остальных; data_dir требует owner rwx, run ровно 0700; socket 0600, uid владельца, один link. При отсутствии каталоги создаются 0700.
- Компоненты пути открываются через openat/dirfd/O_NOFOLLOW; предки принадлежат владельцу либо root, не доступны для чужой записи. Для root-owned sticky каталога /tmp допускается стандартная sticky-модель. Уже небезопасные права, чужой объект, симлинк не исправляются автоматически: запуск UI завершается ошибкой, сокет не создаётся в небезопасном родителе.
- `.ui.lock` 0600, обычный файл, uid владельца, nlink=1, flock EX/NB. Второй listener не удаляет первый. Безопасный старый socket удаляется лишь при ECONNREFUSED; проверка connect неблокирующая, полный backlog трактуется как живой listener. Bind/chmod/unlink закреплены за fd run через /proc/self/fd. Drop удаляет только свой dev/inode.
- 32 активных соединения, включая upgraded WebSocket; сверх лимита соединение закрывается до HTTP. Keep-alive HTTP отключён.
- Заголовки: до 64, buffer 32 KiB, deadline 15 s. HTTP connection/handler: 300 s. Read idle и заблокированная запись: 60 s. WebSocket должен посылать heartbeat чаще 60 s; абсолютный HTTP timeout после upgrade не ограничивает жизнь WS, но IO permit и idle сохраняются.
- Transport ceiling тела 201 MiB, Content-Length выше него даёт 413; streaming body также обёрнут Limited. Существующие route body limits остаются: обычно Axum 2 MiB, загрузка группы 200 MiB + 1 MiB overhead. Ответы файлов не буферизуются transport-слоем заново.
- Transport не журналирует URL, body, пароль, ключ, token или ошибки парсера. Существующие trace/handler logs проекта не расширены.
- Setup и обычный демон используют тот же путь; тест проверяет завершение setup, удаление endpoint и возможность последующего bind. Ошибка открытия socket останавливает запуск обоих UI listeners, не оставляет незаметно только TCP.

Строгие права могут выявить старый data_dir с 0755: его права нужно исправить отдельно до запуска. Transport намеренно не chmod-ит существующие каталоги. Реализация Linux-only (SO_PEERCRED и /proc); для macOS/Windows не объявлена готовность, существующий TCP там сохраняется.

## Проверка

Все cargo-команды: env без HTTP_PROXY/HTTPS_PROXY/ALL_PROXY и lowercase-вариантов, nice -n 15, CARGO_BUILD_JOBS=2, CARGO_TARGET_DIR=/mnt/ssd480/wt/codex/target, XDG_DATA_HOME=/tmp/codex_ds_xdg, --offline. Тесты используют tempfile и подставной AuthState; parity не читает реальные файлы паролей владельца.

- `cargo test --offline --lib web:: -- --test-threads=1`: 34 passed, 0 failed, 8.68 s. Права, опасные родители/симлинки/объекты/права socket, stale recovery, эксклюзивность listener, компонентная проверка чужого uid, connection budget, 413, отсутствие HTML, first setup по socket, TCP Host/Origin/auth, vault status и общие реальные API.
- `cargo test --offline --lib group_client:: -- --test-threads=1`: 10 passed, 0 failed. Включает новый тест отсутствия ключа в UI-копии и сохранения его в daemon body, existing crypto/vector/migration проверки.
- `cargo test --offline --lib closed_ -- --test-threads=1`: 8 passed, 0 failed, 55.64 s. Lock/restart/unlock/rekey, незавершённая миграция, поздний unlock, запрет доступа к закрытому storage и owner endpoint flow.
- После дополнения parity проверены также startup flags и точный 429 StepUp backoff: отдельный `ui_socket_parity_tests` passed (1).
- `git diff --check` проходит. Новый transport форматирован rustfmt. Полный rustfmt всего старого проекта не применялся.
- `scripts/check_spawn.sh` НЕ проходит из-за исходных src/netlayer/rx_pipeline.rs (3 против отсутствующей базы), transport.rs (22 против 21), tunnel_writer.rs (1 против отсутствующей базы). Эти числа сверены с git show исходного HEAD, файлы не менялись. В базе сознательно обновлены только server/first_setup (новые тесты) и ui_socket (одна короткая задача на ограниченное соединение, две тестовые), с пояснением здесь. Общая база не переписана.

Логи: /tmp/codex2-desktop-web-tests.log, /tmp/codex2-desktop-groups-tests.log, /tmp/codex2-desktop-closed-tests.log, /tmp/codex2-desktop-parity-tests.log. Компиляция выводит существующие 163 предупреждения проекта; автоматический cargo fix не запускался.

## Что не проверено

Реальное соединение процесса другого uid (только компонентное сравнение и настоящий peer_cred владельца); production daemon в лаборатории с сетью/телефоном/перезапуском процесса; все API с заполненным реальным состоянием (parity покрывает репрезентативные общие handlers, остальные используют тот же Router); нагрузочный прогон полного 201 MiB chunked upload, slowloris на полных 15/60/300 s и долгий WebSocket; все возможные filesystem race/ACL/mount варианты; работа на не-Linux; GUI в Flutter и звонки. Полный cargo test всего проекта не выполнялся. P3 и удаление веб-интерфейса не выполнялись.
