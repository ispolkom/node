#!/usr/bin/env bash
# Source from repository root. All release entry points use the same inputs.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1577836800}"
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || { echo 'SOURCE_DATE_EPOCH должен быть числом' >&2; exit 2; }
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/mnt/ssd480/wt/codex/target-updater}"
export XDG_DATA_HOME=/tmp/codex_rf_xdg
export TZ=UTC LC_ALL=C
RELEASE_BIN_DIR="$CARGO_TARGET_DIR/release"
REL="$RELEASE_BIN_DIR/yandi-release"
release_root=$(pwd -P)
release_cargo_home=${CARGO_HOME:-$HOME/.cargo}
# Encoded flags avoid whitespace splitting in paths. Replace inherited flags for a fixed recipe.
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=${release_root}=/usr/src/yandi"$'\x1f'"--remap-path-prefix=${CARGO_TARGET_DIR}=/usr/src/yandi-target"$'\x1f'"--remap-path-prefix=${release_cargo_home}=/usr/src/cargo"
ENV_CLEAN=(env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY -u all_proxy -u ALL_PROXY -u no_proxy -u NO_PROXY)
release_build() {
  "${ENV_CLEAN[@]}" nice -n 15 cargo build --release --locked --offline --message-format=json --bin yandi --bin yandi-release | python3 -I tools/release_artifacts.py "$RELEASE_BIN_DIR"
  release_record_identity
}
release_record_identity() {
  RELEASE_IDENTITIES=$(sha256sum "$RELEASE_BIN_DIR/yandi" "$REL")
}
release_check_built() {
  printf '%s\n' "$RELEASE_IDENTITIES" | sha256sum --check --status || { echo "Артефакт изменён после сборки" >&2; return 1; }
}
release_archive() {
  release_check_built
  local version arch name out
  version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
  arch=$(uname -m)
  name="yandi-node-${version}-linux-${arch}"
  out="${YANDI_RELEASE_OUT:-dist/$name}"
  mkdir -p "$out"
  local pkg
  pkg=$(mktemp -d "$out/.pkg.XXXXXX")
  install -m 0755 "$RELEASE_BIN_DIR/yandi" "$pkg/yandi"
  install -m 0755 "$REL" "$pkg/yandi-release"
  for file in LICENSE deploy/yandi-node.service release/yandi-release-pubkey.hex; do
    if [ -f "$file" ]; then install -m 0644 "$file" "$pkg/$(basename "$file")"; fi
  done
  chmod 0755 "$pkg"
  tar --format=ustar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" -C "$pkg" -cf - . | gzip -n -9 > "$out/$name.tar.gz"
  rm -rf "$pkg"
  RELEASE_VERSION=$version RELEASE_NAME=$name RELEASE_OUT=$out
}

# Compare the exact packaged programs to those built and used to sign.
release_check_identity() {
  release_check_built
  local file packed built
  for file in yandi yandi-release; do
    built=$(sha256sum "$RELEASE_BIN_DIR/$file")
    built=${built%% *}
    packed=$(tar -xOf "$RELEASE_OUT/$RELEASE_NAME.tar.gz" "./$file" | sha256sum)
    packed=${packed%% *}
    [[ "$built" = "$packed" ]] || { echo "Артефакт изменён: $file" >&2; return 1; }
  done
}
