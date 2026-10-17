# Файлы для https://yandi.su (привязка ссылок-приглашений к приложению)

Нужны для Android App Links: ссылка `https://yandi.su/i#…` должна открывать приложение YANDI без выбора. **Состояние:** файлы на сайте выложены; сторона приложения (intent-filter с autoVerify в AndroidManifest.xml и обработчик ссылки) ещё не сделана (этап G4 в ROADMAP11/12), поэтому сейчас ссылка пока открывается в браузере.

Положить в корень сайта (`/var/www/yandi_su_usr/data/www/yandi.su/`):
- `.well-known/assetlinks.json` — пакет `com.yandi.yandi_mobile` и SHA-256 отпечаток подписи релиза (если ключ подписи сменится — обновить);
- `i/index.html` — статичная страница для тех, у кого приложения нет (без скриптов, счётчиков и сторонних ресурсов).

Проверка: `curl -s https://yandi.su/.well-known/assetlinks.json` должен вернуть этот JSON с типом `application/json` и без перенаправлений.
