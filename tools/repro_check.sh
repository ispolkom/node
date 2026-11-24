#!/usr/bin/env bash
# Two fresh source trees and release build caches; no signing operation.
set -euo pipefail
cd "$(dirname "$0")/.."
source tools/release_build_env.sh
repro_root=$PWD
repro_work=$(mktemp -d "${YANDI_REPRO_WORK_PARENT:-/mnt/ssd480/wt/codex}/.repro-check.XXXXXX")
repro_saved=0
mkdir -p "$CARGO_TARGET_DIR"
# Keep the user-requested absolute CARGO_TARGET_DIR for both runs; rotate only release caches.
if [ -e "$CARGO_TARGET_DIR/release" ]; then mv "$CARGO_TARGET_DIR/release" "$repro_work/original-release"; repro_saved=1; fi
cleanup() {
  if [ -d "$CARGO_TARGET_DIR/release" ]; then mv "$CARGO_TARGET_DIR/release" "$repro_work/last-release"; fi
  if [ "$repro_saved" = 1 ]; then mv "$repro_work/original-release" "$CARGO_TARGET_DIR/release"; fi
  echo "Материалы проверки: $repro_work"
}
trap cleanup EXIT
# Tracked working-tree bytes (including uncommitted edits), never .git, target, or owner secrets.
git ls-files -z > "$repro_work/files"
tar --null -T "$repro_work/files" -cf "$repro_work/source.tar"
sha256sum "$repro_work/source.tar"
rustc -Vv
cargo -V
printf 'SOURCE_DATE_EPOCH=%s\nCARGO_TARGET_DIR=%s\n' "$SOURCE_DATE_EPOCH" "$CARGO_TARGET_DIR"
for n in 1 2; do
  mkdir -p "$repro_work/src$n" "$repro_work/out$n"
  tar -xf "$repro_work/source.tar" -C "$repro_work/src$n"
done
for n in 1 2; do
  (
    cd "$repro_work/src$n"
    export YANDI_RELEASE_OUT="$repro_work/out$n"
    bash tools/node_release.sh --unsigned
  ) > "$repro_work/build$n.log" 2>&1 || { tail -60 "$repro_work/build$n.log"; exit 1; }
  cp "$CARGO_TARGET_DIR/release/yandi" "$repro_work/out$n/yandi"
  cp "$CARGO_TARGET_DIR/release/yandi-release" "$repro_work/out$n/yandi-release"
  mv "$CARGO_TARGET_DIR/release" "$repro_work/build$n"
done
result=0
for file in yandi yandi-release "yandi-node-${RELEASE_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)}-linux-$(uname -m).tar.gz"; do
  sha256sum "$repro_work/out1/$file" "$repro_work/out2/$file"
  if ! cmp -s "$repro_work/out1/$file" "$repro_work/out2/$file"; then
    result=1
    cmp "$repro_work/out1/$file" "$repro_work/out2/$file" || true
    if [[ "$file" != *.tar.gz ]]; then
      for n in 1 2; do
        readelf -aW "$repro_work/out$n/$file" > "$repro_work/out$n/$file.readelf"
        strings -a "$repro_work/out$n/$file" > "$repro_work/out$n/$file.strings"
      done
      diff -u "$repro_work/out1/$file.readelf" "$repro_work/out2/$file.readelf" > "$repro_work/$file.readelf.diff" || true
      diff -u "$repro_work/out1/$file.strings" "$repro_work/out2/$file.strings" > "$repro_work/$file.strings.diff" || true
      head -80 "$repro_work/$file.readelf.diff" "$repro_work/$file.strings.diff"
    else
      for n in 1 2; do gzip -dc "$repro_work/out$n/$file" > "$repro_work/out$n/archive.tar"; done
      cmp "$repro_work/out1/archive.tar" "$repro_work/out2/archive.tar" || true
    fi
  fi
done
if [ "$result" = 0 ]; then echo 'ВОСПРОИЗВОДИМО'; else echo 'НЕ ВОСПРОИЗВОДИМО: см. cmp/readelf/strings и материалы проверки'; fi
exit "$result"
