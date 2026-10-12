# HANDOFF shell M4 — чат и контакты, 2026-10-12

Рабочая копия `/mnt/ssd480/wt/codex3`, ветка `codex/shell-m4`. Продолжено после прерванного запуска: шаг 1 уже находился в `74374ff`; незакоммиченные правки шага 2 проверены и завершены. Основа — TASK_shell_m4_2026-10-12.md, GUI_SPEC §5–6, §13–14, HANDOFF_shell_m3.md и обновление HANDOFF_shell_m3c.md. Веб-UI сохранён. Ключ закрытой среды и вся криптография остаются у демона.

## Коммиты по шагам

1. `74374ff` — `desktop: add own message editing, local deletion and history clearing (shell M4 step 1)`.
2. `fca3425` — `desktop: add file transfers, previews and explicit downloads (shell M4 step 2)`.
3. `ccfc280` — `desktop: add avatar management and peer profiles (shell M4 step 3)`.
4. `2c55d39` — `desktop: add contact export, own card, trust revocation and tunnels (shell M4 step 4)`.
5. Коммит с названием `desktop: add session language and private message alerts (shell M4 step 5)` включает код оповещений, частичный RU/EN, финальную лабораторную проверку и этот отчёт.

## Выполнено

- Чат: правка своих текстовых сообщений без вложений, локальное удаление своих сообщений, очистка истории с настоящим подтверждением. Статусы Sent/Delivered/Read/Pending/Failed берутся из истории демона; оболочка не придумывает квитанции. Удаление локальное, история второго узла сохраняется.
- Файлы: асинхронный локальный GTK chooser без нового пакета, GTK recent-file history отключена; выбранный файл читается через handle последовательно, без копии/кэша оболочки. Node chunks 700 B, phone chunks 1 MiB; лимиты 200 MiB/1 GiB, пустые файлы отклоняются. Проверяются file_id, received/complete; одна отправка attachment после последнего чанка, без retries. Прогресс означает байты, принятые демоном, а не доставленные получателю. Ошибки лимита/занятости/ID из ответов демона преобразуются в безопасные сообщения без отражения произвольного тела ответа.
- Входящие: status с ограниченным опросом и ручной перепроверкой, растровый предпросмотр в памяти, скачивание в явно выбранный новый файл. Существующие файлы не перезаписываются; неуспешная загрузка удаляет созданный неполный файл. Для image/audio/video — «Сохранить и открыть» системным приложением, для прочих типов — скачивание. Прямых HTTP Image.network, HTML/SVG/скриптовых URI и собственного дешифрования нет. Предпросмотр ограничен 8 MiB; файл до 1 GiB скачивается потоком.
- Аватары: просмотр из `/api/avatar/{short}`, загрузка PNG/JPEG/GIF через daemon multipart, подтверждённое удаление ссылки профиля через PUT `{avatar:""}`. UI лимит <2 MiB учитывает существующий multipart limit. URL из профиля не открывается: путь строится только из проверенного hex ID. Несохранённое имя профиля сохраняется при операциях с аватаром. Данные картинки и ImageCache удаляются при закрытии/блокировке. Профиль собеседника показывает имя/Short ID/online из daemon contact cache; поздние ответы после смены контакта/экрана отбрасываются.
- Контакты: публичная собственная карточка из `/api/peers/trusted`, копирование по явной команде; экспорт ответа `/api/contacts/export` в выбранный `contacts.json`. Отзыв доверия отдельной кнопкой и с подтверждением: отправляется полный ID, затем перечитывается trust list; контакт остаётся. Удаление контакта по-прежнему не отзывает доверие и токен телефона.
- Туннель выбранного узлового контакта: start generic/stop/status, опрос 5 s. Успешный start означает отправленный запрос; active показывается только из ответа status. При ошибке статуса старое состояние очищается; отсутствие туннеля/ошибка запроса явно показаны. Телефонный контакт не получает node tunnel controls. Команды проходят существующий vault preflight и single flight.
- RU/EN: переключатель в настройках, перевод навигации и основных статических кнопок/надписей. Это частичный перевод; формы, динамические описания и часть предупреждений остаются RU, что прямо указано на экране. Тексты пользователя не переводятся.
- Звук/ОС: выключенные по умолчанию opt-in переключатели, «Не беспокоить», системный GDK beep и GNotification без новых зависимостей. Только новые входящие открытого чата; первый снимок, свои сообщения, повторный poll и старые архивные записи не оповещают. В ОС уходят лишь фиксированные «YANDI / Новое сообщение» либо английский эквивалент, без имени/ID/текста. Baseline ограничен текущими 100 message IDs и timestamp; стирается при навигации, hide, logout, lock и disconnect. Настройки живут лишь в памяти сессии; notification отзывается при wipe. Ошибка platform channel отображается в настройках.

## Проверки

Все Flutter/Dart/лабораторные команды — без HTTP_PROXY/HTTPS_PROXY/ALL_PROXY и lowercase вариантов. `--no-pub`; сетевых загрузок, новых пакетов и изменений Rust нет. Использованы имеющиеся SDK/pub/native WebRTC caches.

- `flutter test --no-pub --reporter expanded`: **91 PASS**, прежние M1–M3 и M4. Виджет-тесты чата, файлов, собственного профиля/аватара, контактов/туннелей, RU/EN/оповещений. Проверены daemon rejection, сохранение выбранного файла после ошибки, подтверждения, late responses, отсутствие overwrite, binary limit, baseline/privacy/DND/reset. После локальной обработки ошибки chooser повторный целевой прогон: **12 PASS**.
- `flutter analyze --no-pub`: **No issues found**.
- `flutter build linux --debug --no-pub`: **PASS**, включая GTK chooser/notification bridge; build не коммитится.
- `python3 chaos/desktop_shell_m1.py /mnt/ssd480/wt/codex3/target/debug/yandi`: **PASS** на двух изолированных узлах. Помимо M1–M3 проверены edit/delete/clear, передача 1501 B с Unicode filename A→B и точное сравнение байтов, upload/view/remove avatar reference, peer profile, экспорт JSON и отзыв доверия с сохранением контакта. Это реальный Unix API клиента; живое медиа не открывалось. HOME/XDG/CWD и файлы — только временные fixtures, отдельные user/network/mount namespaces; процессы/данные удалены, боевой узел не затронут.
- `python3 -m py_compile chaos/desktop_shell_m1.py`, `git diff --check`: **PASS**. Cargo не запускался: Rust не менялся, использован ранее собранный локальный демон.

Логи: `/tmp/codex3-m4-step2-tests.log`, `/tmp/codex3-m4-binary-tests.log`, `/tmp/codex3-m4-step3-retest.log`, `/tmp/codex3-m4-step4-tests.log`, `/tmp/codex3-m4-step5-tests.log`, `/tmp/codex3-m4-final-retest.log`, `/tmp/codex3-m4-final-analyze.log`, `/tmp/codex3-m4-final-build.log`, `/tmp/codex3-m4-final-lab.log`.

Ручной GTK chooser, системный viewer, слышимость beep, реальная доставка GNotification рабочим окружением и реальный сетевой tunnel handshake не проверялись. GNotification требует поддержки ОС/интеграции приложения и не даёт подтверждения показа; успех platform bridge не доказывает видимый toast. Phone file transfer пока проверен только клиентскими контрактами, живой телефонный E2E не выполнен.

## Остаток паритета по GUI_INVENTORY перед P3

| Семейство веб-UI | Реализовано / что осталось |
|---|---|
| app.js / общая оболочка | Частичный RU/EN, session sound/DND/OS alerts открытого чата. Остались полный перевод форм/состояний, фоновые unread/alerts вне открытого чата и при hide, рингтоны/online sound, packaging media/licences и desktop integration. Возможности рабочего окружения требуют ручной проверки. |
| index / speed | Основная панель есть; остаются явные copy-ID действия и дополнительные диагностические счётчики. Node/gateway service control — 501, connect/disconnect — stub демона. |
| contacts | Экспорт, own card/copy, отдельный trust revoke и node tunnel controls сделаны. Файлы доступны через открытый чат; отдельного file modal контакта нет. Живой tunnel handshake/ошибки сети требуют E2E. |
| chat | Правка/локальное удаление/очистка, daemon statuses, файлы/progress/watch/raster preview/download, аватары/профиль сделаны. Остались other-devices/add/remove/hide/block, интерактивные приглашения из сообщения, emoji UI, unread/scroll, inline audio/video playback и открытие прочих файлов через системное приложение. Нет retry/resume/cancel API незавершённой загрузки демона; shell прекращает дальнейшие chunks при уходе. |
| groups | Основные роли/правила/invites/transfer/StepUp M2 сохранены. Остались файлы/медиа и preview/download, emoji, phone picker `/api/groups/devices` вместо ручного полного ID, дополнительные presentation/pickers. |
| gateways / relay | Основные сценарии M3 сохранены; actual proxy listener stop остаётся серверным долгом, country flags и форматирование bytes/idle простые. |
| settings | Vault/профиль/аватар/login recovery и сессионные предпочтения есть. Остались рабочие ports/bind/log + restart banner, pairing QR/TTL, paired device revoke, exit policy, upstream proxy pool CRUD/settings/test/health/cap, routes auto/rules/hops/country и personal gateway. |
| voice/video/phone-call | Native WebRTC уже реализован M3C, Linux-сборка работает; это не прежний unavailable fallback M3B. Остались живые voice/video node↔node/phone, устройства/разрешения/NAT/TURN/output/audioLevel E2E, фоновые входящие/overlay, рингтоны/alerts и кнопки звонка из контактов/чата. |
| login/setup | Локальная модель uid намеренная. Browser remembered cookies/logout не нужны; reveal/strength остаются необязательным визуальным остатком. Rebind — CLI-only. |

## Границы и серверный долг

- PUT пустого аватара скрывает ссылку, **не удаляет старый файл** из `~/.yandi/avatars`; upload не рассылает аватар другим узлам, peer profile — contact cache. В оболочке нет обещания удалённого обновления/физического удаления; это отдельная доработка демона.
- Файловый демон сохраняет собственные plaintext staging/downloads; незавершённые chunks удаляются по его существующим правилам. Shell их не копирует и не шифрует самостоятельно. Полный аудит file_id/peer binding, collisions/sparse writes/quota остаётся до release.
- Contact/gateway CRUD по-прежнему может скрывать disk errors; ответы и последующее перечитывание не гарантируют durability. Proxy stop metadata-only также не исправлялся скрытно.
- Wipe удаляет ссылки/контроллеры/preview/cache; физического зануления неизменяемых Dart Strings и внутренних копий codec/OS не гарантирует. Явный экспорт/download/clipboard/open — пользовательские действия вне автоматического хранения shell.

Веб-UI сейчас удалять нельзя: остаются устройства/сопряжение, групповые файлы, расширенная сеть и перечисленные проверки живых сценариев. P3 и удаление веб-UI остаются решением владельца. В коммиты не входят build/, GeneratedPluginRegistrant.java, CODEX_REVIEW_* и временные лабораторные данные.
