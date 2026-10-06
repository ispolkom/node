"""Раздел «Мой выход в интернет через сеть» страницы настроек (node/src/web/ui/settings.html): скрипт раздела берётся из HTML и
выполняется в Node.js с подделкой браузера. Проверяется: режим и страна превращаются в правильный адрес запроса
(auto / hops, с суффиксом страны), неверная страна не отправляется, успешный ответ показывает адрес и пароль.

    python node/tests/ui/route_section_check.py      (нужен node)
"""
import json, os, re, subprocess, tempfile
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
html = open(os.path.join(ROOT, "node/src/web/ui/settings.html"), encoding="utf-8").read()
js = re.search(r"(        // === Мой выход в интернет через сеть: быстро / анонимно \+ страна ===\n.*?\n        \}\)\(\);\n)", html, re.S).group(1)
harness = """
class El { constructor(){ this.value=''; this.textContent=''; this.l={}; } addEventListener(t,f){ (this.l[t]=this.l[t]||[]).push(f); } fire(t){ (this.l[t]||[]).forEach(f=>f()); } }
const ids = {routeMode:new El(), routeCountry:new El(), routeResult:new El(), routeStartBtn:new El()};
global.document = { getElementById: i => ids[i] };
const calls = [];
global.fetch = (p, o) => { calls.push([p, o && o.method]); return Promise.resolve({ json: () => Promise.resolve({ status: 'success', listen_addr: '127.0.0.1:1080', username: 'yandi', password: 'pw' }) }); };
""" + js + """
(async () => { const w = ms => new Promise(r => setTimeout(r, ms)); const out = {};
  ids.routeMode.value = 'auto'; ids.routeCountry.value = ''; ids.routeStartBtn.fire('click'); await w(20); out.fast = calls.slice();
  ids.routeMode.value = 'hops'; ids.routeCountry.value = ' nl '; ids.routeStartBtn.fire('click'); await w(20); out.anon = calls.slice(); out.text = ids.routeResult.textContent;
  ids.routeCountry.value = 'NLD'; ids.routeStartBtn.fire('click'); await w(20); out.bad = [calls.length, ids.routeResult.textContent];
  console.log(JSON.stringify(out)); })();
"""
f = tempfile.mktemp(suffix=".js", dir=os.environ.get("TMPDIR"))
open(f, "w", encoding="utf-8").write(harness)
r = json.loads(subprocess.run(["node", f], capture_output=True, text=True, timeout=60).stdout.strip().splitlines()[-1])
os.unlink(f)
fails = []
def check(name, cond, detail=""):
    print(("OK  " if cond else "FAIL") + " " + name + ("" if cond else f" — {detail}"))
    if not cond: fails.append(name)
check("«Быстро» без страны → /api/socks5/start/auto", r["fast"] == [["/api/socks5/start/auto", "POST"]], r["fast"])
check("«Анонимно» со страной → /api/socks5/start/hops-NL", r["anon"][-1] == ["/api/socks5/start/hops-NL", "POST"], r["anon"])
check("успех показывает адрес и пароль", "127.0.0.1:1080" in r["text"] and "pw" in r["text"], r["text"])
check("неверная страна не отправляется", r["bad"][0] == 2 and "две латинские буквы" in r["bad"][1], r["bad"])
print("RESULT:", "all ok" if not fails else f"{len(fails)} failed")
raise SystemExit(1 if fails else 0)
