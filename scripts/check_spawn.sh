#!/bin/bash
# Не даёт числу «голых» запусков фоновых задач расти.
# Долгоживущие циклы запускаются через `supervisor::supervise` (имя, политика, видимость на /api/kernel/status).
# Обычный `tokio::spawn` допустим для коротких задач (по одной на соединение, на пробу); их число по файлам записано в
# scripts/spawn_baseline.txt и может только УМЕНЬШАТЬСЯ: новый запуск → либо `supervise`, либо осознанное обновление базы
# (`scripts/check_spawn.sh --update`, с пояснением в запросе на слияние).
set -euo pipefail
cd "$(dirname "$0")/.."
cur=$(grep -rc "tokio::spawn(" src --include=*.rs | grep -v ':0$' | sort)
if [ "${1:-}" = "--update" ]; then echo "$cur" > scripts/spawn_baseline.txt; echo "база обновлена"; exit 0; fi
bad=0
while IFS=: read -r file n; do
  base=$(grep "^$file:" scripts/spawn_baseline.txt | cut -d: -f2 || true)
  base=${base:-0}
  if [ "$n" -gt "$base" ]; then echo "НОВЫЕ голые tokio::spawn в $file: было $base, стало $n — используйте supervisor::supervise для долгоживущих задач"; bad=1; fi
done <<< "$cur"
[ "$bad" = 0 ] && echo "голых запусков не прибавилось (всего $(echo "$cur" | awk -F: '{s+=$2} END {print s}'))"
exit $bad
