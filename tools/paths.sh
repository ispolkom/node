# Единые пути выпуска YANDI. Подключается скриптами выпуска (source), сам ничего не делает.
# Корень выпуска (RH) — папка, внутри которой лежит этот репозиторий как «src/»: /mnt/backup/node-release.
# Переопределить: YANDI_RELEASE_HOME=/другой/путь. Описание всей раскладки — README.md в корне выпуска.
RH="${YANDI_RELEASE_HOME:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
[ -f "$RH/README.md" ] && [ -d "$RH/toolchain" ] || { echo "не найден корень выпуска $RH (ожидается node-release/{src,toolchain,site,...})"; return 1 2>/dev/null || exit 1; }
export RH
export CARGO_HOME="$RH/toolchain/cargo" RUSTUP_HOME="$RH/toolchain/rustup"
export JAVA_HOME="$(ls -d "$RH"/toolchain/jdk/jdk-* | tail -1)"
export ANDROID_HOME="$RH/toolchain/android-sdk" ANDROID_SDK_ROOT="$RH/toolchain/android-sdk"
export PUB_CACHE="$RH/toolchain/pub-cache" GRADLE_USER_HOME="$RH/toolchain/gradle"
export FLUTTER_ROOT="$RH/toolchain/flutter-sdk"
export PATH="$CARGO_HOME/bin:$FLUTTER_ROOT/bin:$JAVA_HOME/bin:$PATH"
# Flutter (dart_build) ищет линкер рядом с clang, а в /usr/lib/llvm-14/bin его нет: даём ему каталог-обёртку с clang и ld
SHIM="$RH/toolchain/llvm-shim"
if [ ! -x "$SHIM/clang" ]; then
  mkdir -p "$SHIM"
  for t in clang clang++; do printf '#!/bin/sh\nexec /usr/bin/%s "$@"\n' "$t" > "$SHIM/$t"; chmod +x "$SHIM/$t"; done
  ln -sf /usr/bin/ld "$SHIM/ld"; ln -sf /usr/bin/ld "$SHIM/ld.lld"; ln -sf /usr/bin/ar "$SHIM/ar"
fi
export PATH="$SHIM:$PATH"
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY   # прокси оболочки ломает cargo/flutter/gradle
# куда складываются результаты (всё внутри $RH):
SITE_DIR="$RH/site"             # зеркало сайта yandi.su; update/{deb,android,bootstrap.json} заливаются на сервер как есть
GH_DIR="$RH/github"             # ассеты для GitHub Releases
ARCHIVE_DIR="$RH/archive"       # архивы для заливки и памятки выпусков
PUBLIC_KEYS_DIR="$RH/public-keys"
