# Seed Phrase, Control Word, and Relationship Backup (`.yandibak`)

**The seed phrase completely determines the node.** The same seed, and the same control word if one was used, restores the same node identity (number and keys), its trust state (other nodes and entry-node list), and the entry-list signing key on any machine. The `.yandibak` file is **optional**: it adds data that cannot be derived from the seed, such as paired phones, contacts, and groups. Without the file, phones must be paired again.

* **Seed phrase:** 24 words by default (256 bits); optionally 12 (128 bits) or 18 (192 bits). Only the phrase is displayed.
* **Control word (optional):** one 8–64-character, case-sensitive word that the administrator creates and remembers or stores separately from the seed phrase. It is mixed into the keys with Argon2id, never stored or printed. The same seed with a missing control word produces a DIFFERENT node; if the word is forgotten, the node cannot be restored. The word cannot be changed later because that would create a different node.
* **Node number** (16 characters in groups of four) is shown during setup and before restoration. This makes an incorrect phrase or control word visible; an incorrect word would otherwise silently produce another node. It is not secret and is not written on the seed-phrase sheet. Check the phrase and word without changing anything with `yandi setup --check`.

## What is and is not in the backup

| In the backup | Not in the backup |
|---|---|
| Node keys and certificate (`~/.yandi_keys`, `~/.yandi/tls`) | Messages |
| Paired-device registry, keys, and permissions | Files of any kind, including voice files |
| Contacts and pinned trusted-node keys | Call history and results |
| Groups: name, members, roles, key epoch, and avatars | Delivery/read status and unread counters |
| Closed environment (owner-password shell), exit and routing settings | Group logs, meaning messages stored on the node |
| | Recovery phrase and machine seal `device.seal` |

After restoration, a group returns **without messages stored on the node**. Participants retain what they received earlier; the phone does not erase the keys and feed of a missing group for 30 days. Participants receive “group restored; messages and files on the node were not saved”. Log record numbers continue with a large offset (+1,000,000,000) so new messages do not collide with records already cached on phones.

## Where backups are stored

The owner chooses the location during first setup after the seed and password. The node creates a `yandi-node` directory there, producing a final path such as `…/yandi-node`. A probe file is written and deleted **as the node itself**: if there is no permission, the disk belongs to root, the disk is read-only, or there is no space, the UI reports “I do not have permission to write here” with the reason.

A system-disk location is allowed only after confirmation and a warning to keep backups elsewhere. It can be skipped; the shell and `yandi admin backup status` then keep a reminder. Change the location later in the “Relationship backups” panel under Node settings or with `yandi admin backup set-dir PATH`.

A backup is made when relationships change: contacts, groups, group ownership, adding, promoting, or removing users, including removal after an inactivity timeout. It is made within ten minutes and only when content changed; consecutive edits produce one backup. Guests are not included because they live in node memory; their arrival and departure do not trigger a backup.

Seven automatic backups are kept and older ones overwritten. “Save now” (`yandi admin backup now`) is stored separately, with the latest five retained.

## Creating a backup

* **Button:** shell → Node settings → Relationship backups → Save backup now, or `yandi admin backup now`. The file goes into the backup directory as `yandi-manual-<node>-<number>-<date>.yandibak`; the latest five are retained. If no directory is configured, the shell hides the button until a location is selected.
* **Automatic:** stored in the selected `…/yandi-node` directory. Moving fields such as network state, last activity, and log numbers do not count as content changes. Duplicate the backup to a USB drive or cloud storage, not to the same disk as the node.

## Seed phrase and password

During first setup the node creates a **seed phrase**: 24 English words, or 12 or 18 by choice, from the standard BIP-39 list. The words encode random 256, 128, or 192 bits; the key is derived from those bytes rather than the displayed text, so the screen language makes no difference. The wizard shows the words once, then asks the user to select them in the original order and enter three words by number. It then optionally asks for the control word, password, and backup location. Nothing is written before the password is accepted; closing the window discards the seed and the next startup creates a new one.

* **Seed phrase and control word** restore the node identity, trust state, and keys, and open any `.yandibak` backup made by that node. Store them separately from each other and from backups. Only the seed phrase should be printed on paper.
* **Password** is tied to the installation, as in a wallet. It opens the closed environment on this computer and is not included in the backup. On a new machine, the old installation password **does not work**: use the seed phrase and control word, then create a new password for that machine. The backup is needed only for phones, contacts, and groups.
* **Limits:** five consecutive incorrect passwords close password login, leaving seed-phrase login available. Twenty consecutive incorrect seed phrases erase the closed environment on this computer; the identity remains in the seed. A wrong control word is indistinguishable from guessing and counts as an incorrect phrase. A phrase with words outside the list or an invalid checksum does not count as an attempt. The delay between attempts grows after the fourth error.
* **Honest boundaries:** limits work only in this application instance. They do not protect encrypted files copied in advance; those rely on seed strength and Argon2id, and they cannot prevent disk-snapshot rollback. A backup is protected by the seed phrase and control word: anyone who has both can read backups from that generation. Changing the password does not revoke old backups. Seed rotation is not implemented.

## What opens the backup

The file is encrypted. It opens with the **seed phrase and control word, if one was used**; both the old recovery code and the master password wrapper made for that node are accepted. The file is useless to others. Without the seed phrase and control word, it cannot be read.

## Restoring on a new machine

From the seed, with or without a file, run `yandi setup --restore` or choose “Restore with seed phrase” in the shell. The wizard asks for the seed and control word, if any, shows the node number and asks the owner to confirm it matches their record before writing anything, then asks for a new password and an optional `.yandibak` file or `yandi-node` directory, taking the newest backup. Without a file, identity, trust, and the entry-list signing key return, but phones must be paired again with `yandi admin pairing`.

The shell shows “Restore with seed phrase” on its first screen. The legacy file-only method is:

1. Install the node package, but **do not start or configure** it.
2. `yandi backup inspect file.yandibak` — show number, date, and node without the seed.
3. `yandi backup verify file.yandibak` — verify with the seed without writing.
4. `yandi backup restore file.yandibak` — enter the seed and a new password without echo, then enter `yes`. The command writes keys and relationships, creates a device key for this machine, reseals the closed environment with the new password, and restores the same node identity. If keys already exist, explicit `--force` is required; old files are saved beside them as `*.before-restore-*`.
5. `sudo systemctl start yandi-node`, then log in with the new password.

If restoration is interrupted by power loss or process termination, active files may be partly new. Run it again with `--force`; it completes the replacement and leaves earlier versions beside them as `*.before-restore-*`.

For a risk-free check, `python3 tests/backup/restore_drill.py` starts isolated nodes and checks both methods: on an “empty machine” the same node and contacts return, the old installation password does not work, and the new one opens the environment.

## Backups from older versions

Nodes configured before the “seed as the only secret” transition used a random key root and cannot be restored from the seed alone. The new `yandi setup --restore` rejects such a backup because the node number does not match. Use `yandi backup restore FILE`; it opens the backup wrapper and preserves the old keys. `yandi backup verify|restore` asks for the control word, or accepts `--control-word-stdin`, when the phrase is a seed.

## What this does not promise

* Changes after the last backup, up to ten minutes for automatic backups, and anything outside the backup are not restored.
* If the node, backup, and seed phrase are all lost, the identity cannot be restored. Otherwise anyone could do it.
* A deleted message is never added to a backup, but an old backup may contain relationships (members and devices) that no longer exist. After a device is revoked or banned, the node creates a backup immediately; the directory still retains the latest seven backups.
* Portable human identity, meaning changing nodes without adding contacts again, and moving groups to another node are separate stages.

When restoring from a file, the node checks two values before writing anything: the content fingerprint (SHA-256) against the value shown during backup inspection, and the backup node number against the number derived from the seed and control word. A file changed after inspection and a backup created from a random root rather than this phrase are rejected.
