#!/usr/bin/env python3
# Пробное восстановление копии связей (запускать вручную после сборки: cargo build; python3 tests/backup/restore_drill.py).
"""Пробное восстановление: узел A -> копия -> пустая машина B -> тот же узел? (изолированно, свои порты и каталоги)"""
import os, sys, json, time, shutil, signal, subprocess, urllib.request, urllib.error
BIN = os.environ.get('YANDI_BIN', os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'target', 'debug', 'yandi'))
K = 7
b = 26000 + 100 * K
P = dict(web=b, discovery=b+1, data=b+2, p2p_discovery=b+3, p2p_data=b+4, ws=b+6, http_proxy=b+7, mobile_gateway=b+8, mobile_p2p=b+9)
LOGIN, MASTER = 'drill-login-pass-1', 'drill-master-pass-1'
ROOT = os.environ.get('YANDI_DRILL_DIR', '/tmp/yandi-backup-drill')
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def env_for(d):
    e = dict(os.environ)
    for k in list(e):
        if k.lower().endswith('_proxy'): e.pop(k)
    e.update(HOME=f'{d}/home', XDG_DATA_HOME=f'{d}/home/.local/share', XDG_CONFIG_HOME=f'{d}/home/.config', YANDI_CONFIG=f'{d}/config.yaml',
             YANDI_P2P_DISCOVERY_PORT=str(P['p2p_discovery']), YANDI_P2P_DATA_PORT=str(P['p2p_data']), YANDI_CORE_BIN='self', YANDI_TESTNET='1',
             YANDI_LOCAL_TCP='1', NO_PROXY='127.0.0.1,localhost,::1', no_proxy='127.0.0.1,localhost,::1')
    return e

def prepare(d):
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(f'{d}/home/.local/share', exist_ok=True)
    open(f'{d}/config.yaml', 'w').write(
        f"server: {{bind_address: 127.0.0.1, log_level: info}}\nports: {{discovery: {P['discovery']}, data: {P['data']}, mobile_gateway: {P['mobile_gateway']}, mobile_p2p: {P['mobile_p2p']}, http_proxy: {P['http_proxy']}, web_ui: {P['web']}}}\nnetwork: {{public_ip: 127.0.0.1}}\nws: {{bind: \"127.0.0.1:{P['ws']}\"}}\n")

def start(d):
    log = open(f'{d}/node.log', 'a')
    return subprocess.Popen([BIN], cwd=d, env=env_for(d), stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)

def stop(p):
    try: os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError: pass
    for _ in range(60):
        if p.poll() is not None: return
        time.sleep(0.5)
    os.killpg(p.pid, signal.SIGKILL)

def call(path, method='GET', body=None, cookie=None, raw=False):
    req = urllib.request.Request(f'http://127.0.0.1:{P["web"]}{path}', method=method, data=json.dumps(body).encode() if body is not None else None)
    req.add_header('Content-Type', 'application/json')
    if cookie: req.add_header('Cookie', cookie)
    try:
        r = opener.open(req, timeout=30)
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers
    data = r.read()
    return r.status, (data if raw else json.loads(data or b'{}')), r.headers

def sign_in(first):
    for _ in range(240):
        try:
            st, body, h = call('/api/auth/setup' if first else '/api/auth/login', 'POST',
                {'login_password': LOGIN, 'login_password_repeat': LOGIN, 'master_password': MASTER, 'master_password_repeat': MASTER} if first else {'login_password': LOGIN})
            if st == 200:
                for v in h.get_all('Set-Cookie') or []:
                    if v.startswith('yandi_session='): return v.split(';')[0]
            elif st != 404:
                print('вход не удался', st, body); return None
        except Exception: pass
        time.sleep(0.5)
    return None

def ready(cookie):
    for _ in range(240):
        try:
            st, body, _ = call('/api/peers/trusted', cookie=cookie)
            if st == 200 and isinstance(body, dict) and isinstance(body.get('card'), str): return True
        except Exception: pass
        time.sleep(0.5)
    return False

def node_id(cookie):
    st, body, _ = call('/api/status', cookie=cookie)
    return body.get('node_id') or body.get('short_id')

A, B = f'{ROOT}/A', f'{ROOT}/B'
os.makedirs(ROOT, exist_ok=True)
print('== A: первый запуск'); prepare(A)
pa = start(A)
cookie = sign_in(True)
assert cookie, 'A: вход'
assert ready(cookie), 'A: не готов'
id_a = node_id(cookie)
print('узел A:', id_a)
st, body, _ = call('/api/contacts', 'POST', {'name': 'Тест-Контакт', 'short_id': 'ab' * 8}, cookie)
print('контакт добавлен:', st)
st, body, _ = call('/api/backup/status', cookie=cookie); print('статус копии:', st, {k: body[k] for k in ('counter', 'auto', 'ext')})
st, data, h = call('/api/backup/export', 'POST', {}, cookie, raw=True)
print('export:', st, h.get('Content-Disposition'), len(data), 'байт')
assert st == 200 and len(data) > 500, 'export'
assert 'Тест-Контакт'.encode() not in data and b'node_identity' not in data, 'в файле открытые данные!'
open(f'{ROOT}/node.yandibak', 'wb').write(data)
st, body, _ = call('/api/backup/status', cookie=cookie); print('после копии:', body['counter'], body['last_ok_ms'] > 0)
stop(pa)
print('== B: пустая машина, восстановление')
prepare(B)
r = subprocess.run([BIN, 'backup', 'restore', f'{ROOT}/node.yandibak', '--code-stdin', '--yes', '--port', str(P['discovery'])],
    input=MASTER + '\n', env=env_for(B), capture_output=True, text=True, timeout=300)
print(r.stdout); print(r.stderr[-800:])
assert r.returncode == 0, f'restore вернул {r.returncode}'
pb = start(B)
cookie_b = sign_in(False)
assert cookie_b, 'B: вход по прежнему паролю'
assert ready(cookie_b), 'B: не готов'
id_b = node_id(cookie_b)
print('узел B:', id_b)
st, contacts, _ = call('/api/contacts', cookie=cookie_b)
names = [c.get('name') for c in contacts.get('contacts', [])]
print('контакты B:', names)
stop(pb)
ok = id_a == id_b and 'Тест-Контакт' in names
print('РЕЗУЛЬТАТ:', 'ПРОЙДЕНО — тот же узел и контакты на новой машине' if ok else 'ПРОВАЛ')
sys.exit(0 if ok else 1)
