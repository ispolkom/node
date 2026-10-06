"""Раздел «Выход в интернет через ваш узел» страницы настроек (node/src/web/ui/settings.html): скрипт раздела берётся из HTML и
выполняется в Node.js с подделкой браузера; узел — в памяти (ответы как у /api/exit/settings, сами ответы узла проверяет модульная
проверка exit_policy и node/tests/testnet_exit_test.rs). Проверяется: выбор показан; «Любой узел» — только после подтверждения.

    python node/tests/ui/exit_section_check.py      (нужен node)
"""
import json, os, re, subprocess, tempfile
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
html = open(os.path.join(ROOT, "node/src/web/ui/settings.html"), encoding="utf-8").read()
js = re.search(r"(        // === Выход в интернет через этот узел: кто может ===\n.*?\n        \}\)\(\);\n)", html, re.S).group(1)
harness = """
class El { constructor(){ this.value=''; this.textContent=''; this.l={}; } addEventListener(t,f){ (this.l[t]=this.l[t]||[]).push(f); } fire(t){ (this.l[t]||[]).forEach(f=>f()); } }
const ids = {exitMode:new El(), saveExitBtn:new El(), exitSaved:new El(), exitWarning:Object.assign(new El(), {style:{display:'none'}})};
global.document = { getElementById: i => ids[i] };
let answer = false, asked = 0; global.window = { confirm: () => { asked++; return answer; } };
let mode = 'trusted'; const posts = [];
global.fetch = (p, o) => { if (o && o.method === 'POST') { const b = JSON.parse(o.body); posts.push(b.mode); mode = b.mode; return Promise.resolve({ json: () => Promise.resolve({ ok: true, mode }) }); }
  return Promise.resolve({ json: () => Promise.resolve({ mode, warning: mode === 'off' ? 'мог бы помогать' : null }) }); };
""" + js + """
(async () => { const w = ms => new Promise(r => setTimeout(r, ms)); await w(20); const out = { shown: ids.exitMode.value };
  ids.exitMode.value = 'all'; ids.saveExitBtn.fire('click'); await w(20); out.after_no = [posts.slice(), asked];
  answer = true; ids.saveExitBtn.fire('click'); await w(20); out.after_yes = [posts.slice(), ids.exitSaved.textContent];
  ids.exitMode.value = 'off'; ids.saveExitBtn.fire('click'); await w(20); out.off = [posts.slice(), asked];
  out.warn = [ids.exitWarning.textContent, ids.exitWarning.style.display]; console.log(JSON.stringify(out)); })();
"""
f = tempfile.mktemp(suffix=".js", dir=os.environ.get("TMPDIR"))
open(f, "w", encoding="utf-8").write(harness)
r = json.loads(subprocess.run(["node", f], capture_output=True, text=True, timeout=60).stdout.strip().splitlines()[-1])
os.unlink(f)
fails = []
def check(name, cond, detail=""):
    print(("OK  " if cond else "FAIL") + " " + name + ("" if cond else f" — {detail}"))
    if not cond: fails.append(name)
check("выбор показан (по умолчанию «только доверенные»)", r["shown"] == "trusted", r)
check("«Любой узел» без подтверждения не сохраняется", r["after_no"] == [[], 1], r["after_no"])
check("«Любой узел» с подтверждением сохраняется", r["after_yes"] == [["all"], "Сохранено."], r["after_yes"])
check("«Никто» сохраняется без лишнего вопроса", r["off"] == [["all", "off"], 2], r["off"])
check("при «Никто» владельцу показано предупреждение о взаимности", r["warn"] == ["мог бы помогать", ""], r["warn"])
print("RESULT:", "all ok" if not fails else f"{len(fails)} failed")
raise SystemExit(1 if fails else 0)
