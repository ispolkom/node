# Seed phrase as the node's only root secret: roadmap (owner-approved 2026-10-21)

## Why

Today the node key root is random and the seed is only a lock on `auth.json`. Without the files (after reinstalling or losing the disk), the seed restores nothing, and the `yandi setup` wizard cannot accept an existing seed. The owner's decision is that the seed, with an optional control word, fully determines the node as in a wallet; the `.yandibak` pairing file is an optional supplement for data that cannot be derived from the seed.

## Owner decisions

1. Use 24 words by default; offer 12 / 18 / 24 in the selector. The user chooses the protection level.
2. **Control word**: one word without spaces, entered and remembered by the administrator; at least 8 characters; case-sensitive; entered twice; empty means no word. Without it, the seed does not restore the same node. The word is mixed into key derivation and is never stored. A weak dictionary word produces a warning, not a rejection.
3. **Print only the seed phrase**, and nothing else (no word, reminder, or node number).
4. Restore with the seed, the word if one was used, and a new password. Show the node number and require confirmation before writing. An optional `.yandibak` file (checkbox and “Attach file” button; a path in the console) or a `yandi-node` directory can provide the latest backup. Without the file, the identity, node-list signing key, and trust in the node return; phones, contacts, and groups do not, and phones must be paired again.
5. **Back up pairing data on events.** The location is selected during first setup after the seed and password (it can be skipped, with a reminder). The final path is `…/yandi-node`. The node writes and removes a probe file as its own user; if it lacks permission, show “I cannot write to this location” with the reason; warn when the system disk is selected. Keep seven automatic backups, replacing old ones; keep “Save now” separately (the latest five). Triggers include contacts, groups, group-owner changes, user add/remove/promotion (including timeout-based changes), and manual saves. Guests are not included (they live in memory), so guest arrival and departure do not trigger a backup.
6. All console and shell logic is available in Russian and English.

## Already implemented (verified in code)

* `key_root`: `Seed` (12/18/24, BIP-39), `login::create_keys` (random root), `RootDocument` (device and recovery wrappers), `identity_store` (v3, keys under root HKDF), `looks_like_seed`, and `recovery_secret`.
* `core/identity.rs`: `NodeIdentity::new()` (random keys) and `load_or_create_with_root`.
* `node_vault.rs`: `VaultEnvelope::create_unlocked` (random `id` and `key`), `subkey(domain)` (the node-list signing key is derived from them).
* `node_backup.rs`: `seal/open_with_root/open_with_code`, `run_backup`, `write_to_dir`, `prune_dir` (`KEEP_AUTO=7`), `decide`, state handling, and `finalize_vault`; `backup_api.rs`: `/api/backup/{status,export,config}`, the daily scheduler plus `notify_critical`, and `check_dir` (probe file); `backup_cli.rs`.
* `web/first_setup.rs`: `/api/auth/seed/new`, `/api/auth/setup`, and restore from a backup (`do_restore`, a file is required).
* `admin_cli.rs`: `yandi setup` (always creates a new seed; it does not accept an existing one).
* Shell: `setup_wizard.dart` (four steps, 12/24 selector, seed printing).

## Status (2026-10-21)

Completed: the core (seed → root → identity → environment → node-list signing key), server endpoints and preview, backups (the `yandi-node` directory, write check, ten-minute polling, manual backups separately), the console (`setup`, `--restore`, `--check`, `admin backup`), the shell wizard and the “Pairing backups” panel, and end-to-end tests (`tests/admin_cli_test.rs`). Remaining: Codex review rounds and release.

## Stages

1. **Core (`key_root` + node).** `Seed` 12/18/24; `SeedSecret` = seed + control word → `seed_root` (Argon2id, password = seed‖word, fixed salt domain); node root = HKDF(`seed_root`) instead of random (seed-based path only); node identity keys (X25519, Ed25519) = HKDF(root); `id` and the closed-environment key = HKDF(`seed_root`). Tests: the same seed + word gives the same node number and node-list signing key; another word or seed gives different values; no word works; the old master-password path remains intact.
2. **Server side.** `/api/auth/seed/preview` (derives the node number from the seed and word without writing); `setup` / `vault/setup` / restore accept `control_word`; compare the number with the backup header; open a backup with seed + word.
3. **Backups.** `yandi-node` subdirectory, write-failure reason codes, system-disk warning, candidate list (`/proc/mounts`), ten-minute checks (pick up changes quickly without extra writes), a separate “save now” operation, a “backups are not configured” reminder, and `yandi backup status|now|set-dir|off`.
4. **Console.** `yandi setup`: phrase length, word, node number, backup directory; `yandi setup --restore`, `--check`, `--control-word-stdin`; everything in both languages.
5. **Shell.** `setup_wizard.dart`: “new setup / restore”, 12/18/24 selector, word field, file checkbox and button, backup-directory step, and seed-only printing.
6. **Documents and website.** `BACKUP.md`, `CONSOLE.md`, Debian/Ubuntu pages (ru/en), and README.
7. **Verification and release.** Node, application, and shell tests; four Codex (Sol) rounds; green CI; release; website reconciliation.

## Honest boundaries

* Forgetting the word loses the node irreversibly; changing the word changes the node (new number).
* The word is weaker than the seed: someone with the seed can check guesses against the public node number, so Argon2id is deliberately expensive and we recommend 10+ random characters for the word.
* The seed does not derive phones (tokens), contacts, groups, or chat history; those are in `.yandibak` where applicable (except chat history and files).
* The phone application, with its own seed phrase and environment, is outside this scope.
