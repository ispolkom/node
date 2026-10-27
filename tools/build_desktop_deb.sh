#!/usr/bin/env bash
# Пакет оболочки ПК `yandi-desktop_<версия>_amd64.deb`: значок «YANDI» в меню «Интернет» (раздел Network), запуск оболочки по клику.
#
# Что ставит: сборку Flutter в /opt/yandi/, команду /usr/bin/yandi-desktop, файл меню /usr/share/applications/org.yandi.desktop.desktop
# и значок yandi (48…512 px) в тему hicolor — из desktop/yandi_desktop/linux/packaging/icon_logo.png (логотип из src/web/ui/media).
# Версия — та же, что у пакета узла (метка в бинаре yandi): пакеты выходят парой.
#
# Запуск из корня репозитория, после:
#   (cd desktop/yandi_desktop && flutter build linux --release)  и  cargo build --release --offline
#   bash tools/build_desktop_deb.sh
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."
BUNDLE=desktop/yandi_desktop/build/linux/x64/release/bundle
ICON=desktop/yandi_desktop/linux/packaging/icon_logo.png
[ -x "$BUNDLE/yandi_desktop" ] || { echo "нет $BUNDLE/yandi_desktop: сначала flutter build linux --release"; exit 1; }
[ -f "$ICON" ] || { echo "нет значка $ICON"; exit 1; }
VERSION=$(grep -ao 'YANDI-VERSION-MARK:[0-9]\+\.[0-9]\+\.[0-9]\++g[0-9]\+' target/release/yandi | head -1 | cut -d: -f2)
[ -n "$VERSION" ] || { echo "в target/release/yandi нет метки версии: cargo build --release"; exit 1; }
ARCH=$(dpkg --print-architecture)
PKG="yandi-desktop_${VERSION//+/-}_${ARCH}"   # имя файла без «+»; версия внутри пакета прежняя
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
R="$W/$PKG"
install -d "$R/DEBIAN" "$R/opt/yandi" "$R/usr/bin" "$R/usr/share/applications" "$R/usr/share/doc/yandi-desktop"
cp -a "$BUNDLE/." "$R/opt/yandi/"
# libdartjni.so (нужна libjvm.so) тянется транзитивно от path_provider_android — на ПК она не используется, в пакет не кладём
rm -f "$R/opt/yandi/lib/libdartjni.so"
chmod -R u+rwX,go+rX,go-w "$R/opt/yandi"
# в пакете не должно остаться путей сборочного дерева в RUNPATH плагинов
python3 tools/fix_runpath.py "$R/opt/yandi"
ln -s /opt/yandi/yandi_desktop "$R/usr/bin/yandi-desktop"
# значки: исходник — JPEG под именем .png, поэтому пересобираем настоящие PNG нужных размеров
for s in 48 64 128 256 512; do
  d="$R/usr/share/icons/hicolor/${s}x${s}/apps"
  install -d "$d"
  convert "$ICON" -resize "${s}x${s}" -background none -gravity center -extent "${s}x${s}" "PNG:$d/yandi.png"
done
cat > "$R/usr/share/applications/org.yandi.desktop.desktop" <<DESK
[Desktop Entry]
Type=Application
Version=1.0
Name=YANDI
GenericName=Защищённая связь
Comment=Чаты, файлы и звонки через узел YANDI
Exec=/usr/bin/yandi-desktop
Icon=yandi
Terminal=false
Categories=Network;InstantMessaging;
StartupWMClass=org.yandi.desktop
DESK
chmod 0644 "$R/usr/share/applications/org.yandi.desktop.desktop"
# зависимости: системные библиотеки, которые нужны ВСЕМ ELF бандла (прямые NEEDED, кроме лежащих в самом бандле)
BUNDLED=$(cd "$R/opt/yandi" && find . -name '*.so*' -printf '%f\n' | sort -u)
NEEDED=$(find "$R/opt/yandi" -type f \( -name '*.so*' -o -name yandi_desktop \) -exec env LC_ALL=C readelf -d {} + 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u)
DEPS=""
for lib in $NEEDED; do
  echo "$BUNDLED" | grep -qx "$lib" && continue
  path=$(/sbin/ldconfig -p | awk -v l="$lib" '$1==l {print $NF}' | head -1 || true)
  [ -n "$path" ] || { echo "ОШИБКА: нужной библиотеки $lib нет в системе и нет в бандле"; exit 1; }
  pkg=$({ dpkg -S "$path" 2>/dev/null || dpkg -S "$(readlink -f "$path")" 2>/dev/null || true; } | cut -d: -f1 | head -1)
  [ -n "$pkg" ] || { echo "ОШИБКА: $lib ($path) не принадлежит ни одному пакету"; exit 1; }
  DEPS="$DEPS $pkg"
done
DEPS=$(echo "$DEPS" | tr ' ' '\n' | sort -u | grep -v -E '^(libc6|)$' | tr '\n' ',' | sed 's/,$//; s/,/, /g')
SIZE=$(du -sk "$R/opt" "$R/usr" | awk '{s+=$1} END {print s}')
cat > "$R/DEBIAN/control" <<CTL
Package: yandi-desktop
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: YANDI <yandi-world@outlook.com>
Section: net
Priority: optional
Installed-Size: ${SIZE}
Depends: yandi-node (>= ${VERSION}), libc6 (>= 2.36), libgtk-3-0, libglib2.0-0, libstdc++6${DEPS:+, $DEPS}
Homepage: https://github.com/ispolkom/node
Description: YANDI — оболочка для компьютера
 Окно управления узлом YANDI: чаты, файлы, звонки, группы, настройки. Запускается из меню «Интернет»
 (значок YANDI) или командой yandi-desktop; общается с узлом через локальный сокет пользователя.
CTL
cat > "$R/DEBIAN/postinst" <<'POST'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
POST
cat > "$R/DEBIAN/postrm" <<'POST'
#!/bin/sh
set -e
case "$1" in remove|purge|upgrade|disappear)
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
;; esac
exit 0
POST
chmod 0755 "$R/DEBIAN/postinst" "$R/DEBIAN/postrm"
UNREADABLE=$(find "$R/usr" "$R/opt" -type f ! -perm -0004 -print -quit)
[ -z "$UNREADABLE" ] || { echo "ОШИБКА: файл пакета недоступен обычному пользователю: $UNREADABLE"; exit 1; }
UNSEARCHABLE=$(find "$R/usr" "$R/opt" -type d ! -perm -0001 -print -quit)
[ -z "$UNSEARCHABLE" ] || { echo "ОШИБКА: каталог пакета недоступен обычному пользователю: $UNSEARCHABLE"; exit 1; }
mkdir -p dist
fakeroot dpkg-deb --build --root-owner-group -Zxz "$R" "dist/${PKG}.deb" >/dev/null
echo "готово: dist/${PKG}.deb"
dpkg-deb --info "dist/${PKG}.deb" | sed -n '1,14p'
