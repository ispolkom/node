# Console installation and administration (server without the shell)

The desktop shell is optional: the `yandi-node` is fully managed from the terminal. Console commands are thin clients of the same local socket used by the shell, `~/.local/share/yandi/run/yandi-ui.sock`; they contain no separate setup logic. The socket accepts only the owner of the data directory, checked by uid. Administrative actions require the administrator password just as they do in the shell; the session lasts 15 minutes and the token exists only in process memory.

## Language (Russian / English)
The installer, `yandi setup`, `yandi admin`, and `yandi --help` support both languages. Selection order is: `--lang ru|en`,
the `YANDI_LANG` variable, the saved choice (`~/.config/yandi/lang`), and the system locale
(`LC_ALL`/`LC_MESSAGES`/`LANG`: values beginning with `ru` select Russian; otherwise English). In an interactive terminal, the installer and
`yandi setup` first ask “Выберите язык / Select language [ru/en]” and remember the answer. Yes/no input is strict for the selected keyboard
layout: Russian uses `Д/н` (Cyrillic only), English uses `Y/n` (Latin only). Input from the other layout counts as neither yes nor no, and the
question is repeated with a hint (Latin `Y` on a Russian layout produces `н`). Dangerous actions (erase, unlink, and branch revocation) require
the complete confirmation word (“ДА”/“YES”; erase uses “СТЕРЕТЬ”/“ERASE”). Before running `apt`, installation asks for confirmation
(`Подтвердите установку? [Д/н]` / `Confirm the installation? [Y/n]`); `--yes` and non-interactive runs do not ask. Node-generated error messages
are currently still in Russian.

## Installation
```
bash tools/install.sh            # спросит: 1) только узел (сервер) 2) узел и оболочка ПК
bash tools/install.sh --server   # без оболочки, без вопросов
bash tools/install.sh --desktop  # узел + оболочка
bash tools/install.sh --verify-only --from RELEASE_DIRECTORY   # verify the release only
```
The installer downloads `MANIFEST.json` and its signature, verifies the Ed25519 signature (domain `YANDI-RELEASE-MANIFEST-V1\0`, OpenSSL 3), strictly parses the manifest, checks the hash and size of every required `.deb`, rejects release rollback (the last installed sequence is stored in `~/.local/state/yandi/installed-seq`), and only then installs the packages with `apt-get install` and creates a systemd service for the current user.
The installer refuses to run as root or when the home path contains spaces or `%`, before making any changes (create the service manually for
such a path, using `--no-service`). The signing key is embedded in the script; compare its fingerprint with the website (`--pubkey` selects a
different key). Test the installer itself with `bash tools/test_install.sh RELEASE_DIRECTORY` (manifest, signature, package, wrong-key, and truncation tampering).

## First setup and recovery
```
yandi setup                       # new setup: password, seed phrase (24 words), optional control word, closed environment, backup location
yandi setup --words 12|18|24      # длина сид-фразы
yandi setup --restore             # restore from the seed phrase (and word): same node, trust, and node-list signing key
yandi setup --restore --backup-file FILE_OR_DIRECTORY   # same + pairing backup (.yandibak or …/yandi-node directory)
yandi setup --check               # check the phrase and word: show the node number without changing anything (the node is not needed)
yandi setup --seed-file seed.txt --password-stdin --control-word-stdin --backup-dir /media/iam/FLASH --yes   # без терминала
```
The seed phrase is shown only in the terminal and must be entered again in full; without a terminal, use `--seed-file` (an existing file is not overwritten).
The optional control word is one word (8–64 characters, case-sensitive), entered twice without echo. Without it, the seed alone protects the node;
if the word is forgotten, the node cannot be restored. The node number is shown during setup and before restore (with confirmation there). On
standard input (`--password-stdin`), lines are ordered as follows: phrase (only with `--restore`), control word (only with
`--control-word-stdin`), login password. New setup uses the same order, with the word before the password. Non-interactive `--restore` requires
`--yes` because nobody can confirm the displayed node number. For the “where to store backups” step, see `docs/BACKUP.md`; `--backup-dir PATH`
sets the location without questions, `--no-backup-dir` skips it (a reminder remains), and `--yes` accepts the system-disk risk.
Pairing backups can later be managed with `yandi admin backup status | places | set-dir PATH [--yes] | off | now`.

## Administration
```
yandi admin                       # меню
yandi admin status | devices | alerts | update
yandi admin pairing [owner|user] [--host АДРЕС]       # код и QR сопряжения телефона прямо в терминале
yandi admin device-remove ID                          # отвязать устройство (слово подтверждения «ДА»)
yandi admin bootstrap pubkey | sign ФАЙЛ     # ключ подписи списка входных узлов — только в закрытой среде, файла нет
yandi bootstrap min ФАЙЛ [--node ВЕРСИЯ] [--app ВЕРСИЯ]             # отозвать старые сборки (пусто — снять), затем подписать
yandi admin device-promote ID                         # приглашённое устройство → пользователь (может приглашать само)
yandi admin device-revoke-branch ID                   # отозвать устройство и всех, кого оно пригласило (слово «ДА»)
yandi admin invites show | set --enabled on|off --per-day N --max-live N   # политика приглашений
yandi admin proxies list | add [socks5|http_connect] | remove ID | test | settings --enabled on --strict off --max-clients 5
yandi admin vault status | lock | unlock | change-password | panic-erase   # стирание: слово «СТЕРЕТЬ»
yandi admin node stop | start
yandi admin raw GET /api/status
```
`--socket PATH` selects another socket; `--admin-password-stdin` reads the administrator password from the first standard-input line (for scripts).
For `proxies add|test`, the administrator password is requested BEFORE the proxy list: password first, then one proxy per line, followed by an empty
line. `proxies settings` accepts only `on` or `off`. Node responses are checked strictly: a truncated or non-JSON response is an error.

## What this does not do
- It does not replace a pairing backup: the seed phrase (and control word) restore the node, while phones, contacts, and groups require the `.yandibak` backup; restore with `yandi setup --restore`.
- It does not open firewall ports or configure a router; see `DEPLOYMENT.md` and `deploy/nftables.conf.example`.
- The service is created for the regular user; the installer refuses root (node data is stored in the user's home directory).
