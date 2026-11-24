#!/usr/bin/env python3
"""Независимый HTTP-аудит закрытой среды, только синтетические данные в новом /tmp/r16_*.

Запуск: python3 chaos/audit_round16.py target/debug/yandi.
FAIL означает нарушение проверяемой гарантии, не поломку стенда. Исправления здесь нет.
"""
import concurrent.futures
import json
import os
from pathlib import Path
import random
import socket
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chaos as C


def main():
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/debug/yandi")
    root = tempfile.mkdtemp(prefix="r16_http_", dir="/tmp")
    print("Стенд:", root, flush=True)
    # Если порт занят, не отправляем запросы неизвестному (возможно боевому) сервису.
    for base_port in random.SystemRandom().sample(range(28000, 48000, 100), 200):
        reservations = []
        try:
            for port in range(base_port, base_port + 11):
                for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
                    reservation = socket.socket(socket.AF_INET, kind)
                    reservations.append(reservation)
                    reservation.bind(("0.0.0.0", port))
        except OSError:
            continue
        finally:
            for reservation in reservations:
                reservation.close()
        break
    else:
        raise RuntimeError("для стенда нет свободного блока портов")
    node = C.Node(0, root, binary, base_port=base_port)
    failed = []
    checks = []

    def check(name, condition, details=None):
        checks.append({"name": name, "pass": bool(condition), "details": details})
        print(("PASS " if condition else "FAIL ") + name, details if details is not None else "", flush=True)
        if not condition:
            failed.append(name)

    def api(method, path, body=None, **kwargs):
        return node.api(method, path, body, **kwargs)

    try:
        node.start()
        assert node.login(timeout=45), "стенд не вошёл в веб-интерфейс"
        assert node.ready(timeout=45), "стенд не поднялся"
        # Без сессии HTTP API должен быть закрыт; это положительный контроль, не запрос к боевому узлу.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, req, fp, code, msg, headers, newurl):
                return None

        anonymous = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        try:
            with anonymous.open(node.url("/api/vault/status"), timeout=10) as response:
                code, location = response.status, response.headers.get("Location", "")
        except urllib.error.HTTPError as error:
            code, location = error.code, error.headers.get("Location", "")
        check("status без сессии отклонён", code == 401 or (code in (302, 303, 307) and location.startswith("/login")), (code, location))
        req = urllib.request.Request(node.url("/api/vault/setup"), data=json.dumps({"password": "test owner password"}).encode(), method="POST")
        req.add_header("Content-Type", "application/json")
        req.add_header("Cookie", node.cookie)
        req.add_header("Origin", "http://127.0.0.1:" + str(node.web + 99))
        try:
            with C.OPENER.open(req, timeout=10) as response:
                code = response.status
        except urllib.error.HTTPError as error:
            code = error.code
        check("setup с чужим Origin отклонён", code == 403, code)
        code, body = api("POST", "/api/vault/setup", {"password": "test owner password", "phrase": None})
        assert code == 200 and body["status"]["unlocked"], (code, body)
        check("первичная настройка на синтетическом узле", True)
        code, body = api("POST", "/api/vault/lock", {})
        assert code == 200 and not body["status"]["unlocked"], (code, body)

        # Непрозрачная синтетическая запись: запрещённая операция удаления обязана отказаться ДО разбора истории.
        # Файл создаётся после настройки, чтобы не подменять проверку успешной миграции.
        peer = "22" * 32
        chats = Path(node.dir) / "home/.yandi/chats"
        chats.mkdir(parents=True, exist_ok=True)
        history = chats / ("chat_" + peer + ".enc")
        opaque = b"00112233445566778899aabbccddeeff00112233445566778899aabb\n"
        history.write_bytes(opaque)
        code, body = api("GET", "/api/chats")
        # При изменении имени маршрута сначала сверить server.rs, 404 не засчитывается как защита.
        assert code != 404, "неверное имя маршрута списка чатов"
        check("R16-6: список чатов не раскрывается при Locked", not isinstance(body, dict) or body.get("status") != "success", (code, body))
        code, body = api("POST", "/api/chat/clear/" + peer, {})
        check("R16-5: Locked сохраняет историю через HTTP", history.exists() and history.read_bytes() == opaque, (code, body))

        code, body = api("POST", "/api/upstream-proxies", {"host": "127.0.0.1", "port": 12345, "auth": "password", "user": "test", "password": "test-only-password"})
        check("Locked отклоняет запись прокси", code >= 400, code)
        code, body = api("GET", "/api/upstream-proxies")
        check("R16-9: отклонённая запись не включает прокси", not body.get("enabled", False) and not body.get("proxies"), (code, body))

        # Образ дискового счётчика показывает, что KDF уже начался, но ещё не закончился.
        counter = Path(node.dir) / "home/.local/share/yandi/closed/attempts.json"
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(api, "POST", "/api/vault/unlock", {"password": "test owner password"}, timeout=45)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                state = json.loads(counter.read_text())
                if state["failures"] == 1:
                    break
                assert not future.done(), "стенд не застал KDF"
                time.sleep(0.001)
            else:
                raise AssertionError("стенд не застал KDF за 30 секунд")
            code, body = api("POST", "/api/vault/lock", {})
            assert code == 200 and not body["status"]["unlocked"], (code, body)
            unlock_result = future.result(timeout=45)
        code, body = api("GET", "/api/vault/status")
        check("R16-1: lock выигрывает у более старого HTTP unlock", not body["unlocked"], (code, body, unlock_result))
    finally:
        # Только PID/группа процесса, созданного этим стендом; никаких общих pkill и systemd.
        node.kill()
        Path(root, "audit_results.json").write_text(json.dumps(checks, ensure_ascii=False, indent=2))
        print("FAILS:", failed or "none", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
