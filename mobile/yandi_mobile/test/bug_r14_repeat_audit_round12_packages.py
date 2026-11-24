"""Generate standalone package configuration; never invokes pub/network."""
import json, re
from pathlib import Path
APP=Path(__file__).resolve().parents[1]
def ensure_packages():
    target=Path('/tmp/r14_state/package_config.json')
    candidates=[APP/'.dart_tool/package_config.json',Path('/home/iam/node/mobile/yandi_mobile/.dart_tool/package_config.json')]
    source=next((p for p in candidates if p.is_file()),None)
    if source:
        data=json.loads(source.read_text())
        for item in data['packages']:
            if item['name']=='yandi_mobile':item['rootUri']=APP.as_uri()
            elif not item['rootUri'].startswith(('file:','/')):
                item['rootUri']=(source.parent/item['rootUri']).resolve().as_uri()
    else:
        packages={}
        for path in sorted((Path.home()/'.pub-cache/hosted/pub.dev').glob('*')):
            if not (path/'lib').is_dir():continue
            name=re.sub(r'-\d.*$','',path.name)
            packages[name]={'name':name,'rootUri':path.as_uri(),'packageUri':'lib/','languageVersion':'3.6'}
        data={'configVersion':2,'packages':list(packages.values())}
    target.parent.mkdir(parents=True,exist_ok=True)
    target.write_text(json.dumps(data))
    return target
if __name__=='__main__':print(ensure_packages())
