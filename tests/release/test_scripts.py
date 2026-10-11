#!/usr/bin/env python3
"""No owner phrase or actual build: isolated release script regression fixtures."""
import pathlib, shutil, subprocess, tempfile, unittest
ROOT = pathlib.Path(__file__).resolve().parents[2]

class Scripts(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        shutil.copytree(ROOT / 'tools', self.root / 'tools')
        (self.root / 'Cargo.toml').write_text('version = "1"\n')
        (self.root / 'build/release').mkdir(parents=True)
        (self.root / 'build/release/yandi').write_text('built node')
        (self.root / 'build/release/yandi-release').write_text('#!/bin/bash\nprintf "key\\n"\n')
        (self.root / 'build/release/yandi-release').chmod(0o755)

    def tearDown(self):
        self.tmp.cleanup()

    def shell(self, code, input=''):
        return subprocess.run(['bash', '-c', 'set -euo pipefail\n' + code], cwd=self.root,
                              env={'PATH': '/usr/bin:/bin', 'HOME': str(self.root),
                                   'CARGO_TARGET_DIR': str(self.root / 'build')},
                              input=input, text=True, capture_output=True)

    def test_r2_packaged_identity_is_checked(self):
        result = self.shell('source tools/release_build_env.sh\nrelease_archive\nrelease_check_identity\nprintf evil > "$RELEASE_BIN_DIR/yandi"\nrelease_check_identity')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Артефакт изменён', result.stderr)

    def test_r3_check_never_builds_or_creates_anchor(self):
        result = self.shell('bash tools/node_release.sh --check', 'synthetic\nsynthetic\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'release').exists())
        (self.root / 'release').mkdir()
        anchor = self.root / 'release/yandi-release-pubkey.hex'
        anchor.write_text('key\n')
        result = self.shell('bash tools/node_release.sh --check', 'synthetic\nsynthetic\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(anchor.read_text(), 'key\n')
        self.assertFalse((self.root / 'release/SEQ').exists())
        self.assertFalse((self.root / 'dist').exists())

if __name__ == '__main__':
    unittest.main()
