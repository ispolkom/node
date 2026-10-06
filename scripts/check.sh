#!/bin/bash
# Все проверки проекта одной командой. Запускать из корня:  scripts/check.sh [быстро|полностью]
#   быстро     — сборка и обычные тесты (по умолчанию)
#   полностью  — плюс сетевые тесты на настоящих узлах (около 20 минут)
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:-быстро}"
echo "== сборка =="; cargo build --offline --all-targets 2>&1 | tail -1
echo "== запуски задач =="; scripts/check_spawn.sh
echo "== тесты =="; cargo test --offline --no-fail-fast 2>&1 | grep -E "^test result|FAILED|panicked" | awk '/FAILED|panicked/ {print; bad=1} /^test result/ {p+=$4; f+=$6} END {print "прошло:", p, " упало:", f; exit (f>0||bad)}'
if [ "$mode" = "полностью" ]; then
  echo "== сетевые тесты (настоящие узлы) =="
  cargo test --offline --test testnet_test -- --ignored 2>&1 | grep -E "^test result|FAILED"
  cargo test --offline --test testnet_comms_test -- --ignored 2>&1 | grep -E "^test result|FAILED"
  cargo test --offline --test testnet_rekey_test -- --ignored 2>&1 | grep -E "^test result|FAILED"
  cargo test --offline --test testnet_offers_test -- --ignored 2>&1 | grep -E "^test result|FAILED"
  # тесты ниже требуют публичный адрес у машины (эхо-сервер ставится на него); тесты используют общие порты — по одному
  for t in testnet_exit_test testnet_hops_test testnet_personal_test testnet_rules_test; do
    cargo test --offline --test "$t" -- --ignored --test-threads=1 2>&1 | grep -E "^test result|FAILED"
  done
fi
echo "всё зелёное"
