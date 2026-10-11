#!/usr/bin/env python3
"""Real desktop Dart client / daemon smoke; isolated HOME and offline network.
Usage: python3 chaos/desktop_shell_m1.py /absolute/path/to/yandi [flutter-sdk]
The fixture directory is newly allocated under /tmp and deleted on exit.
"""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time


def main():
    if os.environ.get('YANDI_DESKTOP_M1_LAB') != '1':
        env = dict(os.environ, YANDI_DESKTOP_M1_LAB='1')
        os.execvpe('unshare', ['unshare', '-Urnm', '--', sys.executable, *sys.argv], env)
    # P1 trusts root-owned sticky /tmp. Give the user namespace its own /tmp
    # so real host root is not seen as an unmapped uid (65534).
    subprocess.run(['mount', '-t', 'tmpfs', '-o', 'mode=1777', 'tmpfs', '/tmp'], check=True)
    subprocess.run(['ip', 'link', 'set', 'lo', 'up'], check=True)
    import chaos as C
    binary = str(Path(sys.argv[1]).resolve())
    sdk = Path(sys.argv[2] if len(sys.argv) > 2 else '/home/iam/.local/flutter-sdk')
    probe = Path(__file__).resolve().parents[1] / 'desktop/yandi_desktop/tool/daemon_probe.dart'
    with tempfile.TemporaryDirectory(prefix='yandi-desktop-m1-') as root:
        n = C.Node(0, root, binary, base_port=28700)
        sock = Path(n.dir) / 'home/.local/share/yandi/run/yandi-ui.sock'
        os.environ.pop('CHAOS_NOLOG', None)
        env = dict(os.environ)
        for key in ['HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'ALL_PROXY', 'all_proxy']:
            env.pop(key, None)
        try:
            for phase in ['initial', 'restart']:
                n.start()
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    if not n.alive():
                        raise RuntimeError('daemon stopped before socket became ready: ' + (Path(n.dir) / 'node.log').read_text(errors='replace')[-1500:])
                    try:
                        with socket.socket(socket.AF_UNIX) as conn:
                            conn.connect(str(sock))
                        break
                    except OSError:
                        time.sleep(.25)
                else:
                    raise RuntimeError('socket did not appear within 120s')
                subprocess.run(['nice', '-n', '15', str(sdk / 'bin/dart'), str(probe), str(sock), phase], env=env, check=True, timeout=180)
                n.kill()
                time.sleep(.5)
        finally:
            n.kill()
    print('PASS: desktop M1 real Unix daemon smoke; temporary data removed')


if __name__ == '__main__':
    main()
