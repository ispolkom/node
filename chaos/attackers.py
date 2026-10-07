#!/usr/bin/env python3
"""Hostile participants for the chaos test. Each attack is a function that runs for a given time against one victim address from
INSIDE a network namespace (the attacker's own), and returns a short dict about what it sent. The test then checks that the victim
kept answering, did not grow in memory without bound and did not crash.

Attacks (all against ports a stranger on the network can reach):
  udp_garbage  - random bytes and truncated/oversized datagrams to the discovery and data UDP ports
  udp_mutated  - the same, but mutations of a plausible packet header (type bytes 0x00..0xff, lengths at the limits)
  tcp_flood    - many TCP connections to the node's TCP/TLS port, some silent, some sending garbage after the TLS hello
  slow_tls     - connections that start a TLS handshake byte by byte and then stop (slow-loris)
  ws_garbage   - HTTP/WebSocket upgrade requests with broken headers and huge frames
"""
import os, random, socket, struct, threading, time

import lab as L


def _in_ns(nspath, fn):
    """Run fn() in a thread that has entered the namespace; sockets it makes belong to that namespace."""
    res = {}

    def work():
        import ctypes
        cur = os.open("/proc/thread-self/ns/net", os.O_RDONLY)
        tgt = os.open(nspath, os.O_RDONLY)
        try:
            if L._libc.setns(tgt, L.CLONE_NEWNET) != 0:
                raise OSError(ctypes.get_errno(), "setns")
            res["r"] = fn()
        except Exception as e:
            res["e"] = repr(e)
        finally:
            L._libc.setns(cur, L.CLONE_NEWNET)
            os.close(tgt)
            os.close(cur)

    t = threading.Thread(target=work)
    t.start()
    t.join()
    return res


def udp_garbage(nspath, victim_ip, ports, seconds, pps=3000):
    def run():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setblocking(False)
        end = time.time() + seconds
        sent = 0
        rnd = random.Random(7)
        while time.time() < end:
            for _ in range(200):
                size = rnd.choice([0, 1, 2, 7, 33, 40, 64, 128, 512, 1200, 1472, 1473, 4000, 9000])
                data = bytes(rnd.getrandbits(8) for _ in range(min(size, 1500))) if size else b""
                try:
                    s.sendto(data, (victim_ip, rnd.choice(ports)))
                    sent += 1
                except (BlockingIOError, OSError):
                    pass
            time.sleep(200.0 / pps)
        return {"attack": "udp_garbage", "sent": sent}
    return _in_ns(nspath, run)


def udp_mutated(nspath, victim_ip, ports, seconds, pps=2000):
    def run():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setblocking(False)
        end = time.time() + seconds
        sent = 0
        rnd = random.Random(11)
        while time.time() < end:
            for _ in range(100):
                head = bytes([rnd.choice([0x00, 0x01, 0x02, 0x10, 0x20, 0x50, 0x60, 0xA0, 0xA1, 0xA3, 0xA5, 0xC0, 0xC1, 0xC2, 0xD8, 0xD9, 0xE0, 0xF0, 0xF4, 0xF5, 0xFF])])
                length = struct.pack(">I", rnd.choice([0, 1, 0xFFFF, 0x10000, 0xFFFFFFFF, 0x7FFFFFFF]))
                body = bytes(rnd.getrandbits(8) for _ in range(rnd.choice([0, 4, 32, 64, 300, 1000])))
                try:
                    s.sendto(head + length + body, (victim_ip, rnd.choice(ports)))
                    sent += 1
                except (BlockingIOError, OSError):
                    pass
            time.sleep(100.0 / pps)
        return {"attack": "udp_mutated", "sent": sent}
    return _in_ns(nspath, run)


def tcp_flood(nspath, victim_ip, port, seconds, conns=300):
    def run():
        socks, opened = [], 0
        end = time.time() + seconds
        rnd = random.Random(5)
        while time.time() < end:
            if len(socks) < conns:
                try:
                    s = socket.socket()
                    s.settimeout(2)
                    s.connect((victim_ip, port))
                    opened += 1
                    mode = rnd.random()
                    if mode < 0.3:
                        pass  # silent
                    elif mode < 0.6:
                        s.send(bytes(rnd.getrandbits(8) for _ in range(rnd.choice([1, 5, 64, 1000]))))
                    else:
                        s.send(b"\x16\x03\x01" + bytes(rnd.getrandbits(8) for _ in range(rnd.choice([2, 100, 600]))))
                    socks.append(s)
                except OSError:
                    pass
            else:
                for s in socks[: conns // 10]:
                    try:
                        s.close()
                    except OSError:
                        pass
                socks = socks[conns // 10:]
            time.sleep(0.01)
        for s in socks:
            try:
                s.close()
            except OSError:
                pass
        return {"attack": "tcp_flood", "opened": opened}
    return _in_ns(nspath, run)


def slow_tls(nspath, victim_ip, port, seconds, conns=100):
    def run():
        hello = bytes.fromhex("1603010200010001fc0303") + os.urandom(32) + b"\x00" * 40
        socks = []
        for _ in range(conns):
            try:
                s = socket.socket()
                s.settimeout(2)
                s.connect((victim_ip, port))
                socks.append(s)
            except OSError:
                pass
        end = time.time() + seconds
        pos = 0
        while time.time() < end:
            for s in socks:
                try:
                    s.send(hello[pos % len(hello): pos % len(hello) + 1])
                except OSError:
                    pass
            pos += 1
            time.sleep(1.0)
        for s in socks:
            try:
                s.close()
            except OSError:
                pass
        return {"attack": "slow_tls", "held": len(socks)}
    return _in_ns(nspath, run)


def ws_garbage(nspath, victim_ip, port, seconds):
    def run():
        end = time.time() + seconds
        n = 0
        rnd = random.Random(3)
        reqs = [
            b"GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: !!!\r\nSec-WebSocket-Version: 13\r\n\r\n",
            b"GET / HTTP/1.1\r\n" + b"X-A: " + b"A" * 70000 + b"\r\n\r\n",
            b"\x00" * 100,
            b"POST / HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n",
        ]
        while time.time() < end:
            try:
                s = socket.socket()
                s.settimeout(2)
                s.connect((victim_ip, port))
                s.send(rnd.choice(reqs))
                s.close()
                n += 1
            except OSError:
                pass
            time.sleep(0.02)
        return {"attack": "ws_garbage", "connections": n}
    return _in_ns(nspath, run)


ALL = {"udp_garbage": udp_garbage, "udp_mutated": udp_mutated, "tcp_flood": tcp_flood, "slow_tls": slow_tls, "ws_garbage": ws_garbage}
