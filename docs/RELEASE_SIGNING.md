# Signed node releases (Linux)

The goal is to prevent an installed or downloaded node from being replaced unnoticed. The principle is the same as for the app: the signing key is derived from the developer passphrase and is never stored; the public key is public and pinned by the verifier.

## Components
- `src/release.rs` — release manifest (version, anti-rollback release number, SHA-256 hashes, and file sizes) and an Ed25519 signature over the exact manifest bytes with a domain prefix.
- `yandi-release` — the `pubkey`, `manifest`, `sign`, `verify`, `update`, `rollback`, and `status` tool.
- `tools/node_release.sh` — build, deterministic `tar.gz`, manifest, signature, and self-check.
- `release/yandi-release-pubkey.hex` — the public key, created at first signing and committed.
- `release/SEQ` — the latest release number, committed and incremented by one.

## Creating a release
```
bash tools/node_release.sh          # prompts for the passphrase twice, hidden; creates dist/yandi-node-<version>-linux-<arch>/
bash tools/node_release.sh --check  # checks the passphrase against the pinned key only
```

## Verifying a downloaded release
```
yandi-release verify <release-directory> --pubkey <64 hex from release/yandi-release-pubkey.hex> --min-seq <installed-sequence>
```
The `RELEASE VALID` result means that the manifest signature is valid under the key you trust, the sequence number is not below the specified minimum (equality is allowed when checking an already installed archive), and every file matches its declared size and SHA-256 hash. Do not install any other result.

## What this protects and what it does not
- Protects the supply chain against replacement of an archive on the server or in transit, replacement of individual files, and rollback to an older release.
- Does **not** protect against someone running a modified node on another computer; without hardware attestation, another node cannot detect that.
- Verification must be done by the installer/updater or by a person using `verify`; a running node cannot verify itself.
- Obtain `yandi-release` from a trusted source, such as a previously verified release or a build from source, or the verifier itself could be replaced.

## Keys
There is one signing key, with no hierarchy. If the passphrase is exposed or lost, create a new key and manually pin its public part with all verifiers again. A root key with rotation and revocation is a future step. Windows `.exe` and `.deb` releases with a signed index are also future steps.

## Linux updater

From a trusted `yandi-release` binary:
```sh
yandi-release update --from /path/to/release --min-seq 0 --pubkey <64 hex> --install-dir "$HOME/.local/share/yandi-node"
yandi-release update --from https://server/releases/version --pubkey <64 hex> --install-dir "$HOME/.local/share/yandi-node" --min-seq N
yandi-release status --install-dir "$HOME/.local/share/yandi-node"
yandi-release rollback --install-dir "$HOME/.local/share/yandi-node"
```
Without `--install-dir`, the updater uses `yandi-node` in the user's data directory (`XDG_DATA_HOME`, or `~/.local/share`). Root is not required. The owner configures systemd `ExecStart` to use `<install>/yandi`; the updater does not change or restart the service. After installation it prints `sudo systemctl restart yandi-node`.

Before writing to the installation directory, the updater creates a separate release snapshot and verifies the exact manifest bytes, sizes, and SHA-256 hashes of **all** listed files. The manifest must contain exactly one `.tar.gz`. HTTPS uses the system trusted CAs, no environment proxy, and no redirects; the connect timeout is 10 seconds, the request timeout is 120 seconds, and release download is limited to 300 seconds (synchronous local extraction is not interrupted by that timer). The manifest is limited to 1 MiB, the signature to 256 bytes, one file to 256 MiB, and a release to 512 MiB.

The archive may contain only ustar or simple GNU headers, at most 1 GiB of content, and at most 4096 entries. PAX/GNU long-name extensions, links, devices, duplicate paths, absolute paths, `..`, and hidden top-level names are forbidden. The root must contain a regular `yandi` file. Scripts are not run. Directory and executable modes are 0755; other files are 0644. `yandi` is always 0755, and setuid/setgid bits are not preserved.

Extraction happens in a temporary directory next to the installation. Version directories live in `<install>/.versions/`, and `current` is switched with one `rename`.
The local `.release.json` record lives inside the version, so the visible version/sequence and binary change together. `<install>/previous/` points to the previous version through `current/.previous`; `yandi` and `yandi-release` at the root point to `current`.
Install and rollback operations are serialized with `flock`, and `fsync` runs before switching. An interruption before the switch leaves the active version unchanged; after the switch, the new version is fully prepared. `SIGKILL` may leave temporary and inactive directories because history is not cleaned automatically.

`--min-seq` is required; use 0 for the first installation. When migrating an unmanaged installation, provide its last sequence number. `--current-seq` remains as a compatibility alias.
The updater requires a sequence strictly greater than `max(the specified minimum, the saved max_seq)`. `rollback` is an explicit local owner action to `previous`; the greatest accepted sequence is saved in the new record and continues to prevent network or directory downgrades through `update`.
Reinstalling the same sequence is forbidden, including a sequence already accepted before a rollback. Rollback also switches `current` atomically while saving the replaced version as `previous`.

The root names `yandi`, `yandi-release`, and `previous` must be absent or already be links belonging to this installation; regular files with those names cause rejection.

The installation directory and its parent must belong to the owner and be protected from writes by other users. This does not protect against a local attacker who can modify the updater, key, version directory, or local sequence record.
The `rename`/`fsync` guarantees assume a normal local Linux filesystem; power loss and network filesystems require separate verification. A running process continues to execute the old version until the owner restarts it.

## Reproducibility

`tools/node_release.sh` uses the shared `tools/release_build_env.sh` recipe: `nice -n 15`, `CARGO_BUILD_JOBS=2`, and `cargo build --release --locked --offline`, with proxy variables removed. The default `CARGO_TARGET_DIR` is `/mnt/ssd480/wt/codex/target-updater`; `XDG_DATA_HOME=/tmp/codex_rf_xdg`.
The recipe sets `CARGO_ENCODED_RUSTFLAGS` and replaces inherited flags. Source, target, and Cargo home paths are remapped with `--remap-path-prefix` to stable paths: `/usr/src/yandi`, `/usr/src/yandi-target`, and `/usr/src/cargo`.
`SOURCE_DATE_EPOCH` defaults to `1577836800`, but an explicit fixed value is allowed. Manifest timestamps use the same variable. The ustar archive sorts names, uses uid/gid 0, fixed timestamps and modes, gzip `-n -9`, `TZ=UTC`, and `LC_ALL=C`. Build timestamps and build-directory paths are not embedded in binaries.

```sh
bash tools/node_release.sh --unsigned  # build and archive only, without passphrase, signature, or SEQ
bash tools/repro_check.sh              # two independent full release builds
```
`repro_check.sh` copies tracked **working-tree** files into two different directories (new files must be added to the index first), uses two independent release caches, and preserves the configured absolute `CARGO_TARGET_DIR`. An existing release cache is saved and restored temporarily; do not build concurrently in the same target.
The SHA-256 hashes of the archive, `yandi`, and `yandi-release` are compared. On a mismatch, `cmp`, `readelf`, and `strings` diagnostics are saved. The script does not sign or modify the key or `SEQ`. The two build materials remain in `.repro-check.*` and contain source files and build caches.

Comparison requires identical source, `Cargo.lock`, `SOURCE_DATE_EPOCH`, public key if included in the archive, architecture, rustc/Cargo, system libraries/linker, GNU tar/gzip, and local Cargo cache. `--offline` does not replace a prefilled cache.
This checks builds from different paths on one host; it does not promise identical bytes across distributions or toolchain versions. The owner chooses the sequence number separately. Comparing signed releases requires identical sequence numbers and keys; the reproducibility check itself does not sign.

Run result on 2026-10-11: **REPRODUCIBLE**. Two independent release builds in different directories (6 min 48 s and 6 min 36 s) produced byte-identical `yandi`, `yandi-release`, and archive files; all three `cmp` checks passed. SHA-256:

| Artifact | SHA-256 (identical in both builds) |
| --- | --- |
| `yandi` | `4025cfffcd06270c623e218bac631bf7f921511c526e697351a4270cb007a2c6` |
| `yandi-release` | `886ec9ca6dbfffa4f07624a4e2e9d9d945d307ba85651b8775564c2b8859e35b` |
| `yandi-node-0.2.0-linux-x86_64.tar.gz` | `f424af88ee93690a9151f4dd790c08b102f4d436398b19ceb7545b4ec974747e` |

Environment: x86_64 Linux, rustc/Cargo 1.96.0, LLVM 22.1.2, GCC 12.2.0, GNU ld 2.40, tar 1.34, gzip 1.12, and `SOURCE_DATE_EPOCH=1577836800`. Source worktree and Cargo home paths are absent from the binaries.
This was an unsigned check: the production public key was not yet in the snapshot; adding the pinned key to the archive is a separate step for a future signed build. Materials: `/mnt/ssd480/wt/codex/.repro-check.BJBazF/`.
Library tests: 491 passed with `--test-threads=1`; a parallel run found four `node_vault` test races on shared `KDF_CALLS`, while the serial run passed all tests. CLI installation and rollback checks ran in temporary directories with a synthetic key, including installation of a genuinely generated archive.

## Passphrase normalization v1

`derive_signing_key` applies NFC normalization (pinned to ICU4X 2.2.0), then collapses Unicode whitespace into single spaces. The `YANDI-NODE-RELEASE-KEY-V1` salt and Argon2id parameters are unchanged. The older `node_release.sh` already applied NFC through Python, so keys for releases previously pinned by that script are preserved. A direct old API/CLI call with decomposed Unicode changes the key; this fixes the former API/script mismatch. Such a nonstandard pinned key cannot be rebound silently; the owner must perform an explicit migration. No production key or passphrase was used in this work. ASCII and already-NFC passphrases produce the same KDF result as before.

The signing script holds `release/.lock` from the start of the build through the end of verification. `release/SEQ` is a canonical decimal u64; the number is reserved with an atomic rename and file-and-directory fsync **before signing**. A failure may leave a gap; reusing a number is forbidden. Do not delete or roll back `SEQ` manually. After the first publication, preserve `SEQ` with the pinned public key.
`--check` uses the existing trusted `$CARGO_TARGET_DIR/release/yandi-release`, requires an existing pinned key, and does not build or write anything.

The build, tool execution, and packaging use one artifact directory. Executable paths from Cargo JSON are checked against it; a target override pointing elsewhere is rejected instead of packaging stale binaries. SHA-256 hashes of both programs are recorded immediately after building and checked before packaging and before signing. Binaries inside the archive are checked as well. No release rebuild was run after the fixes: the hashes published below belong to the unsigned run on 2026-10-11 before the fixes and are not hashes of the new code.

## Complete release with one command

`bash tools/release_all.sh --new-keys` creates the first release with a NEW developer passphrase: it writes a new release public key, inserts it into `tools/install.sh`, removes the previous APK certificate, builds the node, shell, and APK, and signs everything. `bash tools/release_all.sh` creates later releases; the passphrase must match the pinned key. `--plan` only checks the passphrase and shows what would be done. The passphrase is entered once, hidden, and never stored. The signing key for the ENTRY-NODE LIST is not part of this scheme; it lives only in the node's private environment (`yandi admin bootstrap pubkey|sign`).
