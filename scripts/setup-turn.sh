#!/usr/bin/env bash
# Сервер звонков для приложения на телефоне: coturn на этом компьютере.
#
# Что делает (запускать один раз, от root):  sudo bash scripts/setup-turn.sh [внешний_адрес]
#   1. ставит пакет coturn;
#   2. пишет /etc/turnserver.conf: порт 3478 (UDP и TCP), временные пароли по схеме «REST API» (общий секрет), релейные порты
#      50000-50199, закрыт доступ к локальным сетям и к самому компьютеру через релей;
#   3. включает и запускает службу;
#   4. открывает порты в nftables (если есть таблица inet filter с цепочкой input) и сохраняет правила в /etc/nftables.d;
#   5. кладёт секрет узлу: ~/.local/share/yandi/turn.json (от имени владельца), после этого узел сам выдаёт телефонам временные пароли.
#
# Звук и видео шифруются между самими телефонами (DTLS-SRTP); сервер звонков видит только зашифрованные пакеты и то, кто с кем соединён.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Запустите от root: sudo bash $0 [внешний_адрес]" >&2
  exit 1
fi

OWNER="${SUDO_USER:-$(logname 2>/dev/null || echo root)}"
OWNER_HOME="$(getent passwd "$OWNER" | cut -d: -f6)"
DATA_DIR="${XDG_DATA_HOME_OF_OWNER:-$OWNER_HOME/.local/share/yandi}"
PORT=3478
RELAY_MIN=50000
RELAY_MAX=50199

# внешний адрес: параметр, иначе первый публичный адрес на интерфейсах
EXT="${1:-}"
if [ -z "$EXT" ]; then
  EXT="$(ip -4 -o addr show scope global | awk '{print $4}' | cut -d/ -f1 | grep -v -E '^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.|100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.)' | head -1)"
fi
if [ -z "$EXT" ]; then
  echo "Не удалось определить внешний адрес. Передайте его параметром: sudo bash $0 185.77.205.3" >&2
  exit 1
fi
echo "Внешний адрес сервера звонков: $EXT"

echo "== 1. coturn"
DEBIAN_FRONTEND=noninteractive apt-get install -y coturn >/dev/null
echo "установлен: $(turnserver --version 2>&1 | head -1)"

echo "== 2. настройка"
SECRET=""
if [ -f "$DATA_DIR/turn.json" ]; then
  SECRET="$(python3 -c "import json,sys;print(json.load(open('$DATA_DIR/turn.json')).get('secret',''))" 2>/dev/null || true)"
fi
[ ${#SECRET} -ge 32 ] || SECRET="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"

cat > /etc/turnserver.conf <<CONF
# создано scripts/setup-turn.sh (YANDI): сервер звонков
listening-port=$PORT
listening-ip=$EXT
relay-ip=$EXT
min-port=$RELAY_MIN
max-port=$RELAY_MAX
fingerprint
use-auth-secret
static-auth-secret=$SECRET
realm=yandi
total-quota=100
no-tls
no-dtls
no-cli
no-multicast-peers
# через релей нельзя дотянуться до локальных сетей и до самого компьютера
denied-peer-ip=0.0.0.0-0.255.255.255
denied-peer-ip=10.0.0.0-10.255.255.255
denied-peer-ip=100.64.0.0-100.127.255.255
denied-peer-ip=127.0.0.0-127.255.255.255
denied-peer-ip=169.254.0.0-169.254.255.255
denied-peer-ip=172.16.0.0-172.31.255.255
denied-peer-ip=192.168.0.0-192.168.255.255
# и то же для IPv6: петля, локальные (ULA) и канальные адреса
denied-peer-ip=::1
denied-peer-ip=fc00::-fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff
denied-peer-ip=fe80::-febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff
# собственный публичный адрес ($EXT) здесь не запрещаем: когда оба телефона идут через этот же релей, адрес собеседника — $EXT:порт релея.
# Но службы, слушающие 0.0.0.0 на этом компьютере, релей на $EXT достаёт мимо внешнего файрвола (пакет идёт через lo). Закрывать это
# правилом nftables на выходе: из портов релея ($RELAY_MIN-$RELAY_MAX) на $EXT — только в порты релея.
log-file=/var/log/turnserver.log
simple-log
CONF
chmod 640 /etc/turnserver.conf
chgrp turnserver /etc/turnserver.conf 2>/dev/null || true
# Debian: служба включается в этом файле
if [ -f /etc/default/coturn ]; then
  sed -i 's/^#\?TURNSERVER_ENABLED=.*/TURNSERVER_ENABLED=1/' /etc/default/coturn
  grep -q '^TURNSERVER_ENABLED=1' /etc/default/coturn || echo 'TURNSERVER_ENABLED=1' >> /etc/default/coturn
fi

echo "== 3. защита релея от доступа к службам этого компьютера"
# служба на 0.0.0.0 этого компьютера не должна доставаться через релей по собственному внешнему адресу (пакет идёт через lo мимо
# входного файрвола): из портов релея на $EXT — только в порты релея. Правило в СВОЕЙ таблице, независимо от чужого файрвола.
# Ставится ДО запуска coturn одной транзакцией (nft -f) и повторяется при каждом запуске службы (drop-in ExecStartPre): после
# перезагрузки релей не поднимется без правила, а ошибка применения не оставляет его работающим.
OUT_RULE="ip daddr $EXT udp sport $RELAY_MIN-$RELAY_MAX udp dport != $RELAY_MIN-$RELAY_MAX drop"
NFT_FILE=/etc/yandi-turn.nft
if command -v nft >/dev/null; then
  NFT_BIN=$(command -v nft)
  cat > "$NFT_FILE" <<NFT
table inet yandi_turn
flush table inet yandi_turn
table inet yandi_turn {
  chain output {
    type filter hook output priority 0; policy accept;
    $OUT_RULE
  }
}
NFT
  chmod 644 "$NFT_FILE"
  if ! "$NFT_BIN" -f "$NFT_FILE" || ! "$NFT_BIN" list chain inet yandi_turn output | grep -q "udp sport $RELAY_MIN-$RELAY_MAX"; then
    echo "не удалось поставить защиту релея от самого себя: coturn остановлен и выключен"
    systemctl stop coturn 2>/dev/null || true
    systemctl disable coturn >/dev/null 2>&1 || true
    exit 1
  fi
  mkdir -p /etc/systemd/system/coturn.service.d
  printf '[Service]\nExecStartPre=+%s -f %s\n' "$NFT_BIN" "$NFT_FILE" > /etc/systemd/system/coturn.service.d/yandi-nft.conf
  systemctl daemon-reload
  echo "защита релея поставлена (таблица inet yandi_turn) и применяется при каждом запуске coturn."
elif [ "${YANDI_TURN_ALLOW_SELF_RELAY:-0}" != "1" ]; then
  echo "nft не найден: без правила на выходе релей достаёт службы этого компьютера по его внешнему адресу ($EXT)."
  echo "Установите nftables или поставьте эквивалентное правило своим брандмауэром и запустите скрипт с YANDI_TURN_ALLOW_SELF_RELAY=1."
  systemctl stop coturn 2>/dev/null || true
  exit 1
fi

echo "== 4. служба"
systemctl enable coturn >/dev/null 2>&1 || true
systemctl restart coturn
sleep 2
systemctl is-active --quiet coturn && echo "coturn работает" || { echo "coturn не запустился:"; journalctl -u coturn -n 20 --no-pager; exit 1; }

echo "== 5. брандмауэр"
if command -v nft >/dev/null && nft list chain inet filter input >/dev/null 2>&1; then
  add_rule() { nft list chain inet filter input | grep -q -- "$1" || nft add rule inet filter input $1; }
  add_rule "tcp dport $PORT accept"
  add_rule "udp dport $PORT accept"
  add_rule "udp dport $RELAY_MIN-$RELAY_MAX accept"
  echo "порты $PORT (TCP и UDP) и $RELAY_MIN-$RELAY_MAX (UDP) открыты сейчас."
  echo "ЧТОБЫ ОНИ ОСТАЛИСЬ ПОСЛЕ ПЕРЕЗАГРУЗКИ добавьте в цепочку input в /etc/nftables.conf строки:"
  echo "    tcp dport $PORT accept"
  echo "    udp dport $PORT accept"
  echo "    udp dport $RELAY_MIN-$RELAY_MAX accept"
else
  echo "nftables с таблицей inet filter не найден: откройте порты $PORT (TCP и UDP) и $RELAY_MIN-$RELAY_MAX (UDP) своим брандмауэром."
fi
echo "== 6. секрет для узла"
# каталог создаётся от имени владельца (иначе при чистой установке он достанется root, и узел не сможет в него писать)
runuser -u "$OWNER" -- mkdir -p "$DATA_DIR"
chmod 700 "$DATA_DIR"
python3 - <<PY
import json, os
p = "$DATA_DIR/turn.json"
with open(p, "w") as f:
    json.dump({"port": $PORT, "secret": "$SECRET", "host": "$EXT"}, f)
os.chmod(p, 0o600)
PY
chown "$OWNER":"$(id -gn "$OWNER")" "$DATA_DIR/turn.json"
echo "готово: $DATA_DIR/turn.json"
echo
echo "Проверка с телефона в мобильной сети: порт $PORT на $EXT должен отвечать. Узел подхватывает turn.json сам, перезапуск не нужен."
