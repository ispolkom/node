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

    def test_r4_seq_is_strict_durable_and_never_reused(self):
        (self.root / 'release').mkdir()
        seq = self.root / 'release/SEQ'
        for bad in ['-1', '01', '1+2', '', '18446744073709551615', '1\n2']:
            seq.write_text(bad)
            result = self.shell('python3 -I tools/release_seq.py release')
            self.assertNotEqual(result.returncode, 0, bad)
            self.assertEqual(seq.read_text(), bad)
        seq.write_text('7\n')
        for expected in [8, 9]:
            result = self.shell('python3 -I tools/release_seq.py release')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(seq.read_text(), f'{expected}\n')
        seq.unlink()
        (self.root / 'release/yandi-release-pubkey.hex').write_text('pinned')
        self.assertNotEqual(self.shell('python3 -I tools/release_seq.py release').returncode, 0)
        seq.symlink_to(self.root / 'Cargo.toml')
        self.assertNotEqual(self.shell('python3 -I tools/release_seq.py release').returncode, 0)

    def test_r4_signing_holds_exclusive_lock(self):
        # A stub build must see the signing lock already held.
        env = self.root / 'tools/release_build_env.sh'
        env.write_text(env.read_text() + '\nrelease_build() { flock -n release/.lock true && exit 99; exit 42; }\n')
        result = self.shell('bash tools/node_release.sh')
        self.assertEqual(result.returncode, 42, result.stderr)

if __name__ == '__main__':
    unittest.main()
