#!/usr/bin/env python3
"""Capture actual Linux windows with Xvfb and ImageMagick, without a daemon.
First build: flutter build linux --no-pub --target tool/design_preview.dart
"""
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
SCENES = {
    'offline': 'offline', 'login': 'login', 'setup': 'setup', 'unlock': 'unlock',
    'dashboard': 'Главная', 'chat': 'Чат', 'groups': 'Группы',
    'contacts': 'Контакты', 'gateways': 'Шлюзы', 'relay': 'Relay',
    'settings': 'Настройки', 'calls': 'Звонки',
}

def capture(name, width, height):
    env = dict(os.environ, GSETTINGS_BACKEND='memory')
    output = ROOT / 'HANDOFF' / 'screenshots'
    output.mkdir(parents=True, exist_ok=True)
    with open(output / 'native_preview.log', 'a') as log:
        process = subprocess.Popen(
            [str(ROOT / 'build/linux/x64/release/bundle/yandi_desktop'), SCENES[name]],
            cwd=ROOT, env=env, stdout=log, stderr=log)
        try:
            window = subprocess.check_output([
                'xdotool', 'search', '--sync', '--onlyvisible', '--pid', str(process.pid),
                '--name', 'YANDI'], timeout=15, text=True).splitlines()[0]
            subprocess.run(['xdotool', 'windowsize', window, str(width), str(height)], check=True)
            subprocess.run(['xdotool', 'windowmove', window, '0', '0'], check=True)
            time.sleep(2)
            # Open the first contact/group so the native screenshot also covers messages.
            if name == 'chat':
                subprocess.run(['xdotool', 'mousemove', '92', '258', 'click', '1'], check=True)
                time.sleep(.5)
            if name == 'groups':
                subprocess.run(['xdotool', 'mousemove', '115', '350', 'click', '1'], check=True)
                time.sleep(.5)
            subprocess.run(['import', '-window', 'root', str(output / f'after_{name}_{width}x{height}.png')], check=True)
        finally:
            process.terminate()
            process.wait(timeout=10)

if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--capture':
        capture(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    else:
        for width, height, names in [(1280, 800, SCENES), (900, 600, ['chat', 'groups', 'settings'])]:
            for name in names:
                subprocess.run(['xvfb-run', '-a', '-s', f'-screen 0 {width}x{height}x24',
                    sys.executable, __file__, '--capture', name, str(width), str(height)], check=True)
                print(f'{name}: {width}x{height}', flush=True)
