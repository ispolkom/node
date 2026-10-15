#!/usr/bin/env bash
# Сборка пакета узла `yandi-node_<версия>_amd64.deb` из готового release-бинаря (без службы systemd: юнит ставится отдельно).
#
# Запуск из корня репозитория, после `cargo build --release --offline`:
#   bash tools/build_deb.sh
# Результат: dist/yandi-node_<версия>_amd64.deb и dist/SHA256SUMS. Подпись релиза — отдельным шагом (tools/sign_release.sh, когда он
# появится): сам пакет не подписывается, подписывается список контрольных сумм.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/release
for f in yandi yandi-keys; do [ -x "$BIN/$f" ] || { echo "нет $BIN/$f: сначала cargo build --release --offline"; exit 1; }; done
# версия — та, что вшита в сам бинарь (метка YANDI-VERSION-MARK): пакет и узел обязаны называть себя одинаково
VERSION=$(grep -ao 'YANDI-VERSION-MARK:[0-9]\+\.[0-9]\+\.[0-9]\++g[0-9]\+' "$BIN/yandi" | head -1 | cut -d: -f2)
[ -n "$VERSION" ] || { echo "в бинаре нет метки версии: пересоберите cargo build --release"; exit 1; }
ARCH=$(dpkg --print-architecture)
# имя файла без «+» (панели хостингов ломаются на таком имени); версия внутри пакета прежняя
PKG="yandi-node_${VERSION//+/-}_${ARCH}"
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
R="$W/$PKG"
install -d "$R/DEBIAN" "$R/usr/bin" "$R/usr/share/doc/yandi-node"
install -m 0755 "$BIN/yandi" "$R/usr/bin/yandi"
install -m 0755 "$BIN/yandi-keys" "$R/usr/bin/yandi-keys"
strip --strip-unneeded "$R/usr/bin/yandi" "$R/usr/bin/yandi-keys" 2>/dev/null || true
for d in DEPLOYMENT.md README.md; do [ -f "$d" ] && install -m 0644 "$d" "$R/usr/share/doc/yandi-node/$d"; done
SIZE=$(du -sk "$R/usr" | cut -f1)
cat > "$R/DEBIAN/control" <<CTL
Package: yandi-node
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: YANDI <yandi-world@outlook.com>
Section: net
Priority: optional
Installed-Size: ${SIZE}
Depends: libc6 (>= 2.36), libgcc-s1, libopus0, libssl3 (>= 3.0)
Recommends: coturn, nftables
Homepage: https://github.com/ispolkom/node
Description: YANDI — узел сети личных соединений
 Узел P2P-сети: чаты, файлы, звонки, группы, выход в интернет для доверенных узлов.
 Пакет ставит только бинарники (/usr/bin/yandi, /usr/bin/yandi-keys). Служба systemd и правила брандмауэра
 настраиваются отдельно (см. /usr/share/doc/yandi-node/DEPLOYMENT.md). Данные узла лежат в каталоге пользователя
 (~/.local/share/yandi) и пакетом не затрагиваются.
CTL
mkdir -p dist
fakeroot dpkg-deb --build --root-owner-group -Zxz "$R" "dist/${PKG}.deb" >/dev/null
( cd dist && sha256sum "${PKG}.deb" > SHA256SUMS )
echo "готово: dist/${PKG}.deb"
dpkg-deb --info "dist/${PKG}.deb" | head -12
