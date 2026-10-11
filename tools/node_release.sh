#!/usr/bin/env bash
# Подписанный выпуск ноды (Linux): сборка → архив → манифест → подпись ключом из фразы → самопроверка.
#
# Фразу вводит сам разработчик (скрыто, дважды); она не сохраняется, ключ живёт только в памяти процесса. Ключ подписи — Ed25519,
# выводится из фразы Argon2id с собственной солью (это НЕ ключ APK, даже при той же фразе). Открытый ключ лежит в репозитории:
# release/yandi-release-pubkey.hex (закоммитить при первой подписи); при следующих подписях фраза проверяется по нему.
# Номер выпуска (против отката) — release/SEQ, увеличивается на единицу при каждой подписи (закоммитить).
#
# Запуск из корня репозитория:  bash tools/node_release.sh
#   --unsigned  сборка/архив без ввода фразы, подписи и изменения release/SEQ
#   --check  только проверить фразу по открытому ключу, без сборки
#   --show   вводить фразу видимо (проверить раскладку/опечатку; делать только когда никто не смотрит)
set -euo pipefail
cd "$(dirname "$0")/.."
SHOW=0; CHECK=0; UNSIGNED=0
for a in "$@"; do case "$a" in --show) SHOW=1;; --check) CHECK=1;; --unsigned) UNSIGNED=1;; *) echo "неизвестный ключ: $a"; exit 2;; esac; done

PUB=release/yandi-release-pubkey.hex
SEQFILE=release/SEQ
source tools/release_build_env.sh
if [ "$CHECK" = 1 ]; then
  [ "$UNSIGNED" = 0 ] || { echo '--check несовместим с --unsigned'; exit 2; }
  [ -f "$PUB" ] && [ ! -L "$PUB" ] || { echo 'Нет закреплённого ключа; --check не создаёт его'; exit 1; }
  [ -x "$REL" ] || { echo "Сначала соберите доверенный инструмент: $REL"; exit 1; }
else
  release_build
fi
if [ "$UNSIGNED" = 1 ]; then
  release_archive
  echo "Архив без подписи: $RELEASE_OUT/$RELEASE_NAME.tar.gz"
  exit 0
fi

echo "Фраза разработчика: не меньше 12 слов и 60 символов. Это фраза подписи ВЫПУСКОВ НОДЫ."
if [ "$SHOW" = 1 ]; then
  read -r -p "Фраза: " P1; read -r -p "Ещё раз: " P2
else
  read -rs -p "Фраза: " P1; echo
  read -rs -p "Ещё раз: " P2; echo
fi
PHRASE_IN=$(printf '%s\n%s\n' "$P1" "$P2")
unset P1 P2

PUBKEY=$(printf '%s\n' "$PHRASE_IN" | "$REL" pubkey) || { echo "фраза отвергнута"; exit 1; }
if [ "$CHECK" = 0 ]; then mkdir -p release; fi
if [ ! -f "$PUB" ]; then
  echo "$PUBKEY" > "$PUB"
  echo "Первая подпись: создан $PUB — его нужно закоммитить (он открытый)."
elif [ "$(cat "$PUB")" != "$PUBKEY" ]; then
  echo "Фраза не та: ключ не совпадает с release/yandi-release-pubkey.hex. Остановлено."
  exit 1
else
  echo "Фраза верна: ключ совпадает с закреплённым."
fi
if [ "$CHECK" = 1 ]; then echo "Проверка завершена без сборки и записи."; exit 0; fi

SEQ=$(( $(cat "$SEQFILE" 2>/dev/null || echo 0) + 1 ))
release_archive
release_check_identity
VERSION=$RELEASE_VERSION NAME=$RELEASE_NAME OUT=$RELEASE_OUT

"$REL" manifest "$OUT" "$VERSION" "$SEQ" --pubkey "$PUBKEY" "${NAME}.tar.gz"
printf '%s\n' "$PHRASE_IN" | "$REL" sign "$OUT"
unset PHRASE_IN
"$REL" verify "$OUT" --pubkey "$PUBKEY" --min-seq "$SEQ"
echo "$SEQ" > "$SEQFILE"
echo
echo "Готово: $OUT/"
ls -l "$OUT"
echo "Закоммитить: $SEQFILE${PUB:+ и $PUB (если создан)}."
