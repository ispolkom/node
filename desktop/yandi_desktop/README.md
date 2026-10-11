# YANDI desktop · P2 M1

Нативная Linux-оболочка существующего демона, без WebView и собственного хранилища данных.
Первичная настройка, локальный вход по uid, панель и закрытая среда. Остальные экраны P2 и удаление веб-интерфейса P3 не входят в M1.

## Офлайн-сборка и тесты

Из этого каталога, Flutter 3.38.9 / Dart 3.10.8:

```bash
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter pub get --offline
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter test
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter analyze
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 /home/iam/.local/flutter-sdk/bin/flutter build linux --debug --no-pub
```

`--no-pub` после отдельного `pub get --offline` исключает сетевое разрешение пакетов.
Зависимости времени исполнения — только Flutter SDK. Все зависимости тестов уже есть в pub-кэше.
`build`, `.dart_tool`, engine/plugin outputs и машинные настройки исключены из Git.

## Запуск

```bash
./build/linux/x64/debug/bundle/yandi_desktop
./build/linux/x64/debug/bundle/yandi_desktop --socket /absolute/test/data/run/yandi-ui.sock
```

Приоритет: `--socket PATH` / `--socket=PATH`, затем `YANDI_UI_SOCKET`, затем `$XDG_DATA_HOME/yandi/run/yandi-ui.sock` или `$HOME/.local/share/yandi/run/yandi-ui.sock`.
Не указывайте URL вместо пути. Сокет и демон должны принадлежать вашему пользователю. Без демона окно показывает «Нет связи с демоном»; интернет оболочке не требуется.

На Unix-сокете браузерный пароль/cookie не требуются: владелец допускается демоном по uid. «Войти» открывает локальную панель; пароль закрытой среды вводится отдельно. Перепривязка identity выполняется существующим `yandi-keys recover`, а форма восстановления пароля использует `/api/auth/recover`.

## Проверка с настоящим демоном

Из корня репозитория:

```bash
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY nice -n 15 python3 chaos/desktop_shell_m1.py /absolute/path/to/current/yandi
```

Нужны `unshare`, `mount`, `ip`, доступные user namespaces и текущий бинарь с P1. Скрипт создаёт отдельные user/network/mount namespaces и tmpfs `/tmp`, поднимает только loopback, задаёт временные HOME/XDG через существующий лабораторный Node и запускает тот же Dart UnixApi. Проверяет setup, status/nodes/speed, vault setup/lock/unlock/change/recover/auto-lock, HTTP 501 и запертое состояние после рестарта. Временные данные удаляются. Это RPC-интеграция; GTK-формы с настоящим демоном вручную отдельно.
Не запускайте `tool/daemon_probe.dart` на сокете владельца: он меняет тестовые пароли и настройки.

## Память и границы

Нет хранения паролей, cookies, токенов, истории, настроек или API-ответов на диске. GPU shader disk cache отключён в runner; GTK settings backend — memory. Секретные поля скрыты, без автодополнения, подсказок и персонализации IME. Контроллеры очищаются при отправке, скрытии, выходе и блокировке. UTF-8 копии и буфер запроса зануляются, ответ ограничен 1 МиБ и его byte buffer также зануляется. Dart/Flutter editing, JSON и системный IME используют неизменяемые String и внутренние копии, физическое стирание всей памяти не гарантируется. ОС может иметь swap/core dumps, debug-сборка имеет VM service; это не сборка для распространения.

Опрос vault — 1 с, status — 5 с, speed — 2 с, nodes — 10 с, без наложения циклов. При скрытии/потере фокуса отменяются запросы, очищаются формы и останавливается таймер; после возврата перечитываются auth/vault. Внешняя блокировка видна при ближайшем успешном poll; задержка ответа может увеличить интервал. При неподтверждённой собственной блокировке данные не возвращаются до явного повтора. Автоблокировку выполняет демон; оболочка не меняет её политику при скрытии/выходе.

Клиент не повторяет POST и не выводит серверные тела в логи/ошибки (они могут содержать секрет). Отображаются безопасный русский контекст и HTTP-код. Ошибки 401/429/501 различаются. Лимит соединений — 6, общий deadline RPC — 15 с. Проверка server SO_PEERCRED со стороны Dart не реализована; клиент использует защищённый P1 путь без symlink конечного сокета и без TCP fallback.

Подробности проверки и непроверенное: `docs/HANDOFF_desktop_shell_P2M1_2026-10-11.md` в корне репозитория.
