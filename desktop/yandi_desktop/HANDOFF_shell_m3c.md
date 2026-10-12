# HANDOFF shell M3, шаг C — 2026-10-12

Ветка `codex/shell-m3c`, база `origin/main` = `bbfeeadf17b07f5c45c551311650ad3a159ed839`.

Реализован `WebRtcCallMedia`, подключённый по умолчанию в экране звонков. Нативные RTCPeerConnection/getUserMedia, Unified Plan, видео 640×480, fallback audio и recvonly с русским уведомлением. Узлы: SDP-объект и candidate, voice/video типы старого WS; телефоны: SDP-строка и cand через существующий call_signaling. Два прежних Google STUN; временные TURN credentials телефона из `/api/mobile/call/turn`, UDP/TCP. SDP и credentials не сохраняются и не выводятся адаптером в лог.

Mute и camera управляют enabled треков; при отсутствии камеры её кнопка отключена. Локальный/удалённый RTCVideoView и уровень микрофона из getStats.audioLevel (если нативный backend отдаёт эту метрику). Без локальных устройств создаются recvonly transceivers. Существующий координатор сохраняет дедлайны, busy, epoch, очереди SDP/ICE и завершение при уходе/блокировке. `close()` идемпотентен: отключает callbacks/таймер, очищает renderer sources, останавливает локальные/удалённые треки, освобождает streams, renderers, PC. Поздние ресурсы после close тоже освобождаются; `released` позволяет дождаться завершения cleanup. Ошибка одной операции освобождения не отменяет остальные попытки.

Проверено:
- `flutter pub get --offline`: зависимости найдены в кэше, сетевых загрузок не было.
- `flutter test --no-pub --reporter expanded`: 76 PASS (70 прежних и 6 новых). Новые тесты используют подставные native objects: node/phone SDP и ICE, камера/микрофон, STUN/TURN config, remote preview, fallback/recvonly, stop/dispose всех ресурсов даже при ошибке stop, позднее открытие PC/renderers/getUserMedia, stats и поздний callback после закрытия. Прежние coordinator/widget/lock тесты сохранены.
- `flutter analyze --no-pub`: No issues found.
- `flutter build linux --debug --no-pub`: PASS с libwebrtc; плагин через ldd разрешает libwebrtc.so из bundle/lib.

**Реальная медиа-проверка выполнена только до уровня сборки.** Живой микрофон, камера, звук на выходе, изображение, разрешения ОС, связь узел↔узел/узел↔телефон, фактический TURN/NAT/DTLS-SRTP и реальная audioLevel-метрика не проверялись. Успешные unit tests с подставными объектами не доказывают работоспособность живого звонка.

## Библиотека и размер

- Flutter plugin: `flutter_webrtc 1.6.2+hotfix.4` (точная зависимость в pubspec.yaml).
- Native linux-x64: `libwebrtc.m150.7871.03`.
- ZIP SHA-256: `0acd09c7ae1bc9e61733337d6a4467010725b78734534124a4370f623c8a16db`.
- ZIP `libwebrtc-linux-x64-release.zip`: 9 252 100 байт (8,82 MiB).
- `bundle/lib/libwebrtc.so`: 25 401 008 байт (24,22 MiB).
- Debug bundle: около 166,4 MB / 158,69 MiB (сумма размеров файлов). Это распакованный debug-пакет, release и сжатый дистрибутив не собирались.
- Результат: `/mnt/ssd480/wt/codex3/desktop/yandi_desktop/build/linux/x64/debug/bundle/`.

## Повторить офлайн-сборку

Нужны уже установленные Flutter SDK и Linux build dependencies, pub-кэш пакета и его зависимостей. Ничего не устанавливать из сети. Перед CMake обязательно проверить архив: без него plugin CMake пытается скачивать библиотеку.

```bash
cd /mnt/ssd480/wt/codex3/desktop/yandi_desktop
webrtc_cache_dir=/home/iam/.pub-cache/hosted/pub.dev/flutter_webrtc-1.6.2+hotfix.4/third_party
mkdir -p "$webrtc_cache_dir/downloads"
cp /mnt/ssd480/libwebrtc_cache/dl/libwebrtc-linux-x64-release.zip "$webrtc_cache_dir/downloads/"
sha256sum "$webrtc_cache_dir/downloads/libwebrtc-linux-x64-release.zip"
# Продолжать только при совпадении SHA-256 выше.
unset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy
/home/iam/.local/flutter-sdk/bin/flutter pub get --offline
# При ошибке pub get остановиться, не запускать online pub get.
/home/iam/.local/flutter-sdk/bin/flutter test --no-pub
/home/iam/.local/flutter-sdk/bin/flutter analyze --no-pub
/home/iam/.local/flutter-sdk/bin/flutter build linux --debug --no-pub
```

CMake распаковывает ZIP в third_party/libwebrtc, если каталога ещё нет. Pubspec.lock по существующим правилам проекта игнорируется. build/, GeneratedPluginRegistrant.java и CODEX_REVIEW_* в коммиты не включены.
