# YANDI Deployment Guide

## Минимальные требования хоста

- Linux (kernel 5.x+, тестировалось на Debian 12).
- Rust 1.75+ для сборки (если не используется prebuild бинарь).
- Открытые порты: 9000/UDP (discovery), 10000/UDP (data), 9001/UDP (P2P discovery), 9998/UDP (P2P data), 8443/TCP (WS-over-TLS), 9999/TCP (Web UI).

## ⚠️ Обязательный sysctl на каждом хосте

Без этого под нагрузкой UDP-пакеты дропаются в ядре (RCVBUF capped).

```bash
sudo sysctl -w net.core.rmem_max=67108864
sudo sysctl -w net.core.wmem_max=67108864

cat <<EOF | sudo tee /etc/sysctl.d/99-yandi.conf
net.core.rmem_max=67108864
net.core.wmem_max=67108864
EOF
sudo sysctl -p /etc/sysctl.d/99-yandi.conf
```

При запуске нода логирует warning если `rmem_max < 4MB` — это маркер что sysctl не применён.

## Автозапуск узла (systemd)

Юнит лежит в [`deploy/yandi-node.service`](deploy/yandi-node.service): узел стартует вместе с системой, перезапускается при сбое и пишет в журнал.

```bash
pkill -x yandi                     # если узел запущен вручную: порты заняты
sudo cp deploy/yandi-node.service /etc/systemd/system/yandi-node.service
sudo systemctl daemon-reload
sudo systemctl enable --now yandi-node
systemctl status yandi-node
journalctl -u yandi-node -f
```

После новой сборки (`cargo build --release`) достаточно `sudo systemctl restart yandi-node`. Пользователь и пути в юните (`iam`, `/home/iam/node`) под этот компьютер; на другом поправьте `User=`, `Group=`, `WorkingDirectory=`, `ExecStart=`.

Порты узла для телефонов и звонков (в `/etc/nftables.conf`): 8444/TCP (вход для приложения), 3478 TCP+UDP и 50000-50199 UDP (сервер звонков, ставится `sudo bash scripts/setup-turn.sh <внешний адрес>`).

## Сборка

```bash
cargo build --release
# бинарь: ./target/release/yandi
```

## Запуск anchor-ноды (заглубленный публичный сервер)

```bash
./target/release/yandi --anchor --jurisdiction NL
```

Опционально:
- `--ws-bind 0.0.0.0:443` — кастомный bind WS-server'а (default 8443).
- `--jurisdiction XX` или `--my-jurisdiction XX` — ISO-3166 alpha-2 self-claim для exit-policy.

## Запуск mobile-клиента (легковесный peer)

```bash
./target/release/yandi --mobile \
  --anchor-url wss://anchor.example.com:443/ \
  --anchor-fp <TLS-sha256-fingerprint-hex>
```

Опционально:
- `--anchor-store /path/to/paired_anchors.json` — кастомный путь.
- `--import-pairing '<QR-string>'` — однократно импортировать pairing-payload из QR.
- `--exit-jurisdiction DE` — узел предпочитает выходы в указанной стране.

## Bootstrap-список

Новый узел находит сеть по **подписанному списку входных узлов** (`bootstrap/bootstrap.json` в репозитории): он скачивается при запуске и раз в 6 часов, проверяется закреплённым ключом и сохраняется на диск. Подробно и как владельцу обновлять список — [`bootstrap/README.md`](bootstrap/README.md).

Дополнительно владелец узла может положить рядом с программой локальный файл `nodes/bootstrap.json` (подписи не требует; в репозиторий он не попадает, см. `.gitignore`):
```json
{
  "auto_connect": true,
  "connect_on_startup": true,
  "nodes": [
    { "name": "MY-NODE-1", "address": "203.0.113.10:9000", "jurisdiction": "XX", "enabled": true, "role": "entry",
      "ed25519_fingerprint": "<64 hex: ключ узла; если указан, подмена узла по этому адресу не пройдёт>" }
  ]
}
```

## Файлы конфигурации/состояния

Все живут в `~/.yandi/`:

- `config.yaml` (опционально) — общая конфигурация (см. `[ws] bind` и др.).
- `paired_clients.json` — anchor хранит SessionToken'ы спаренных мобилок (0o600).
- `paired_anchors.json` — mobile хранит список anchor'ов с preference + session.
- `tls/cert.pem`, `tls/key.pem` — self-signed TLS-identity (генерится автоматически).
- `node_identity_*.json` — Ed25519/X25519 keypair'ы (генерится при первом запуске).

## Web UI

После старта web-интерфейс на `http://127.0.0.1:9999/`. Endpoints для pairing:
- `GET /pair/qr` — SVG-QR с PairingPayload.
- `GET /pair/qr.json` — JSON для копипаста.
- `POST /pair/issue` — anchor выдаёт SessionToken по pubkey.

## HTTP/SOCKS5 proxy

После запуска ноды через CLI команду:
```
> proxy <SHORT_ID>     # HTTP proxy → ::8080, трафик через peer SHORT_ID
> socks5 <SHORT_ID>    # SOCKS5 proxy → ::1080
> peers                # список known peers
```

## Verification

После запуска убеждаемся:
- `Bootstrap complete: N connected` — bootstrap-ноды подцепились.
- `Session vN established with peer: <id>` — ECDH-сессия установлена.
- `ws-server 🌐 listening on 0.0.0.0:8443` — TLS-fingerprint показан.
- Нет warning'ов про `rmem_max`.

## Stress-test (после первого деплоя)

```bash
# 8 параллельных downloads через прокси
for i in 1 2 3 4 5 6 7 8; do
  curl -s -o /dev/null -x http://127.0.0.1:8080 \
    -w "#$i: %{speed_download} B/s\n" \
    --max-time 60 https://speed.cloudflare.com/__down?bytes=20000000 &
done; wait

# UDP-drops:
awk 'NR>1 && $13!="0" { port=strtonum("0x"substr($2,index($2,":")+1)); printf "port=%d drops=%s\n", port, $13 }' /proc/net/udp | sort -u
```

Ожидаемо: суммарно ~60-80 Mbps (зависит от ISP-канала), drops=0.

## Troubleshooting

| Симптом | Причина | Лечение |
|---|---|---|
| Browser white-screen под нагрузкой | RCVBUF мал, kernel дропает UDP | `sudo sysctl rmem_max=67108864` |
| `Bootstrap complete: 0 connected` | Bootstrap-ноды недоступны | Проверь файрвол: 9000-10000/UDP |
| `ws-server bind 8443 failed: Permission denied` | Хочется на :443 без root | Используй `--ws-bind 0.0.0.0:8443` или `setcap cap_net_bind_service=+ep` на бинарь |
| Медленный proxy при отсутствии шейпинга | Подсчитать `/proc/net/udp` drops; если растут — sysctl | См. выше |

---

См. также:
- `NETWORK_EVOLUTION_PLAN.md` — архитектурный roadmap.
- `NETWORK_EVOLUTION_STATUS.md` — текущее состояние реализации.
- `FUTURE_IDEAS.md` — backlog архитектурных идей (PEP, swarming, mask-mode и т.д.).
