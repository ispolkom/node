# Console installation and administration (server without the shell)

The desktop shell is optional: the `yandi-node` is fully managed from the terminal. Console commands are thin clients of the same local socket used by the shell, `~/.local/share/yandi/run/yandi-ui.sock`; they contain no separate setup logic. The socket accepts only the owner of the data directory, checked by uid. Administrative actions require the administrator password just as they do in the shell; the session lasts 15 minutes and the token exists only in process memory.

## Language (Russian / English)
Установщик, `yandi setup`, `yandi admin` и справка `yandi --help` работают на двух языках. Выбирается по порядку: ключ `--lang ru|en`,
переменная `YANDI_LANG`, сохранённый выбор (`~/.config/yandi/lang`), язык системы (`LC_ALL`/`LC_MESSAGES`/`LANG`: начинается с `ru` — русский,
иначе английский). В терминале без явного выбора установщик и `yandi setup` в самом начале спрашивают «Выберите язык / Select language [ru/en]»
и запоминают ответ. Ответ «да/нет» принимается строго по раскладке выбранного языка: по-русски `Д/н` (только кириллица), по-английски `Y/n`
(только латиница); ответ другой раскладки не засчитывается ни как «да», ни как «нет», вопрос повторяется с подсказкой (латинская `Y` на русской
раскладке печатает «н»). Опасные действия (стирание, отвязка, отзыв ветки) просят ввести слово целиком («ДА»/«YES», стереть — «СТЕРЕТЬ»/«ERASE»).
Установка перед запуском `apt` просит подтверждение (`Подтвердите установку? [Д/н]` / `Confirm the installation? [Y/n]`); `--yes` и запуск без
терминала вопросов не задают. Сообщения самого узла (ошибки в ответах) пока приходят по-русски.

## Installation
```
bash tools/install.sh            # спросит: 1) только узел (сервер) 2) узел и оболочка ПК
bash tools/install.sh --server   # без оболочки, без вопросов
bash tools/install.sh --desktop  # узел + оболочка
bash tools/install.sh --verify-only --from КАТАЛОГ_ВЫПУСКА   # только проверить выпуск
```
The installer downloads `MANIFEST.json` and its signature, verifies the Ed25519 signature (domain `YANDI-RELEASE-MANIFEST-V1\0`, OpenSSL 3), strictly parses the manifest, checks the hash and size of every required `.deb`, rejects release rollback (the last installed sequence is stored in `~/.local/state/yandi/installed-seq`), and only then installs the packages with `apt-get install` and creates a systemd service for the current user.
От root и при домашней папке с пробелами или `%` установщик отказывает ДО любых изменений (для такой папки службу создают вручную, `--no-service`). Ключ подписи вшит в скрипт; сверьте его отпечаток с сайтом (`--pubkey` задаёт другой). Проверка самого
установщика: `bash tools/test_install.sh КАТАЛОГ_ВЫПУСКА` (подмена манифеста, подписи, пакета, чужой ключ, обрезка).

## First setup and recovery
```
yandi setup                       # новая настройка: пароль, сид-фраза (24 слова), необязательное контрольное слово, закрытая среда, место для копий
yandi setup --words 12|18|24      # длина сид-фразы
yandi setup --restore             # восстановление по сид-фразе (и слову): тот же узел, то же доверие, тот же ключ подписи списка узлов
yandi setup --restore --backup-file ФАЙЛ_ИЛИ_ПАПКА   # то же + копия связей (.yandibak или папка …/yandi-node)
yandi setup --check               # проверить фразу и слово: показать номер узла, ничего не делая (узел не нужен)
yandi setup --seed-file seed.txt --password-stdin --control-word-stdin --backup-dir /media/iam/FLASH --yes   # без терминала
```
Сид-фраза показывается только в терминале и проверяется повторным вводом целиком; без терминала нужен `--seed-file` (существующий файл не перезаписывается).
Контрольное слово — одно слово (8–64 знака, регистр важен), вводится дважды без эха, необязательно: без него сид один защищает узел; забыв слово, узел вернуть
нельзя. Номер узла выводится при настройке и перед восстановлением (там с подтверждением). Порядок строк на стандартном вводе (`--password-stdin`): фраза
(только `--restore`), контрольное слово (только `--control-word-stdin`), пароль входа. Это же порядок и при новой настройке (слово до пароля). Без терминала `--restore` требует `--yes` (подтвердить номер узла некому, он выводится выше). Шаг «где хранить копии»: см. `docs/BACKUP.md`; `--backup-dir ПУТЬ` задаёт
место без вопросов, `--no-backup-dir` пропускает шаг (останется напоминание), `--yes` принимает риск системного диска.
Копии связей позже: `yandi admin backup status | places | set-dir ПУТЬ [--yes] | off | now`.

## Administration
```
yandi admin                       # меню
yandi admin status | devices | alerts | update
yandi admin pairing [owner|user] [--host АДРЕС]       # код и QR сопряжения телефона прямо в терминале
yandi admin device-remove ID                          # отвязать устройство (слово подтверждения «ДА»)
yandi admin bootstrap pubkey | sign ФАЙЛ     # ключ подписи списка входных узлов — только в закрытой среде, файла нет
yandi bootstrap min ФАЙЛ [--node ВЕРСИЯ] [--app ВЕРСИЯ]             # отозвать старые сборки (пусто — снять), затем подписать
yandi admin device-promote ID                         # приглашённое устройство → пользователь (может приглашать само)
yandi admin device-revoke-branch ID                   # отозвать устройство и всех, кого оно пригласило (слово «ДА»)
yandi admin invites show | set --enabled on|off --per-day N --max-live N   # политика приглашений
yandi admin proxies list | add [socks5|http_connect] | remove ID | test | settings --enabled on --strict off --max-clients 5
yandi admin vault status | lock | unlock | change-password | panic-erase   # стирание: слово «СТЕРЕТЬ»
yandi admin node stop | start
yandi admin raw GET /api/status
```
`--socket ПУТЬ` — другой сокет; `--admin-password-stdin` — пароль администратора первой строкой стандартного ввода (для скриптов). Для
`proxies add|test` пароль администратора запрашивается ДО списка прокси: порядок строк — пароль, затем прокси по одному в строке, пустая
строка. `proxies settings` принимает только `on` или `off`. Ответ узла проверяется строго: оборванный или не-JSON ответ — ошибка.

## What this does not do
- Не заменяет резервную копию связей: сид-фраза (и контрольное слово) возвращают узел, а телефоны, контакты и группы — только копия `.yandibak`; восстановление: `yandi setup --restore`.
- Не открывает порты брандмауэра и не настраивает роутер: см. `DEPLOYMENT.md` и `deploy/nftables.conf.example`.
- Служба создаётся для обычного пользователя; запуск установщика от root отказывает (данные узла лежат в домашней папке).
