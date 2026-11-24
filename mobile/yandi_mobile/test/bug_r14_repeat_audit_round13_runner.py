"""All new R13 tests, standalone Dart, no pub/network/Flutter/ports."""
import os,re,subprocess
from pathlib import Path
from bug_r14_repeat_audit_round12_packages import ensure_packages
APP=Path(__file__).resolve().parents[1];AUDIT=Path('/mnt/ssd480/yandi_codex_audit');packages=ensure_packages()
env=os.environ.copy()
for k in ['http_proxy','https_proxy','HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','all_proxy']:env.pop(k,None)
env['XDG_DATA_HOME']=str(Path('/tmp/r14_state/data'))
commands=[('core',['/home/iam/.local/flutter-sdk/bin/cache/dart-sdk/bin/dart',f'--packages={packages}','test/bug_r14_repeat_bug_r13_core.dart'])]
commands.extend((n,['python3',f'test/bug_r14_repeat_audit_round13_{n}_runner.py'])for n in ['services','db','lifecycle'])
passed=failed=0
for name,cmd in commands:
 r=subprocess.run(cmd,cwd=APP,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);(AUDIT/f'r14_repeat_{name}.log').write_text(r.stdout);print(name+':\n'+r.stdout,flush=True)
 m=re.search(r'RESULT (\d+) PASS / (\d+) FAIL',r.stdout)
 if not m:raise SystemExit('Infrastructure failure: '+name)
 passed+=int(m[1]);failed+=int(m[2])
print(f'ALL SUITES: {passed} PASS / {failed} FAIL',flush=True)
raise SystemExit(int(failed>0))
