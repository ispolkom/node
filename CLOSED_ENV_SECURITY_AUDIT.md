# YANDI Node Closed-Environment Security Audit

## Scope and baseline

This audit covers the Linux implementation in the repository at commit `24c2794`.
The review follows actual initialization, locking, unlock, migration, storage, web,
mobile, and local Unix-socket paths. It does not treat file permissions as a
replacement for encryption.

The audit is limited to synthetic test data. No live node profile, recovery phrase,
private key, token, or published release artifact was opened or modified.

## Executive result

The Linux daemon owns the closed environment. Desktop is a client of the daemon and
does not create a second vault. After a restart, the owner-level environment remains
locked even when the daemon's device-level key is available.

The following properties are confirmed by code and unit tests:

- owner data is encrypted with keys derived from the closed-environment vault;
- device/service data uses separate HKDF domains derived from the machine working key;
- locking drops the owner vault and revokes consumer keys;
- the daemon can continue approved service work without opening owner chat history;
- malformed, damaged, or unreadable encrypted files are not silently replaced with
  empty state by the audited storage components;
- the Unix UI socket requires the daemon's effective UID and private directory/socket
  ownership and modes;
- the same daemon and vault are used with or without Desktop.

One defense-in-depth defect was confirmed and fixed: `/api/chat/*` was not included in
the web locked-content guard. The chat storage still rejected reads without its key,
so this was not a demonstrated plaintext disclosure, but it could return a route-level
success/error response instead of the consistent locked response and left future chat
handlers outside the policy boundary. `src/web/server.rs` now covers `/api/chat` and
has a regression test.

No other confirmed Linux cryptographic bypass was found in the available static and
unit-test audit. Physical systemd, reboot, GUI, and multi-process attack testing are
reported as `NOT TESTED` below.

## Two protection levels

### Device/service level

`src/web/auth.rs` loads the machine/device-wrapped root from `auth.json` and the device
key material. This level is intentionally available to the daemon after boot. It is
used for node identity loading, TLS/network operation, device-to-node delivery, and
approved service areas.

This level must not be confused with the owner vault. Possession of the working key
does not derive the owner vault key.

### Owner/closed level

`src/closed_env.rs` opens `data_dir()/closed` through `EnvelopeStore`, while
`src/node_vault.rs` keeps the data key only in a `Zeroizing` session. The password and
recovery phrase unwrap the same data key through Argon2id and AES-GCM. Each consumer
receives an HKDF-separated subkey using its domain name.

On startup, `main.rs` calls `ClosedEnv::open`, registers consumers, calls
`start_service`, and does not call owner `unlock`. `ClosedEnv::lock` clears the owner
vault and locks non-service consumers. The generation/epoch checks prevent an unlock
that raced with a lock from reopening the environment.

## Storage classification

| Data | Location / implementation | Protection while at rest | Available while locked | Result |
|---|---|---|---|---|
| Owner vault envelope, attempts, anchor | `~/.local/share/yandi/closed/{envelope.json,attempts.json,anchor.json}`; `node_vault/store.rs`, `store/gate.rs` | Password/recovery-wrapped random data key; authenticated envelope and rollback anchor | No | PASS |
| Personal chat history, statuses, outbox | `~/.yandi/chats/*.enc`; `communication/storage.rs` | AES-GCM with HKDF key from closed consumer key | No; storage returns an error | PASS |
| PC groups, group keys, private group logs | `data_dir()/groups_pc/{state.enc,*.log}`; `group_client.rs` | AES-256-GCM with a domain key | No owner key; service group state is deliberately available under its service key | PASS with documented service boundary |
| Files and transfer chunks | `data_dir()/files`; `file_vault.rs` | AES-256-GCM containers, authenticated file/block metadata | No on Linux when closed is configured | PASS |
| PC identity as a phone contact | `data_dir()/mobile_self.enc`; `mobile_self.rs` | AES-256-GCM under the service/owner domain key; public bundle is separate | Public keys only; private signing/X25519 keys no | PASS |
| Reactions and local contact overrides | `chat_reactions.enc`, `local_overrides.enc`; `sealed_json.rs` | AES-GCM sealed JSON | No | PASS |
| Proxy credentials/settings | `data_dir()/upstream_proxy.enc`; `upstream_proxy.rs` | AES-GCM sealed settings | No | PASS |
| Complaints and measures | `data_dir()/complaints.enc`; `complaints.rs` | AES-GCM sealed state | Available to approved service/admin paths under a service key | PASS by current service policy; privacy risk accepted and documented |
| Bootstrap signing key | No persistent private-key file; `bootstrap/signer.rs` consumer | In-memory key derived from owner domain | No | PASS |
| Device-to-PC pending mail | `data_dir()/pc_inbox.json`; `mobile_api.rs` | File contains E2E ciphertext blobs plus sender/time metadata, not plaintext messages | Queue count and delivery processing only | PASS; metadata remains visible to the daemon |
| Login hash and machine-wrapped root | `auth.json`; `web/auth.rs` | Login hash; root encrypted by device-derived key | Device/service level | Intended service data |
| Node identity private keys | Linux key-root storage under `~/.yandi_keys`; `core/identity.rs`, `key_root` integration | Root/device key protection; private material is zeroized in memory | Daemon needs it for networking | Intended service data |
| TLS private key/certificate | TLS identity storage used by `netlayer/tls_cert.rs` | Private file permissions and TLS identity storage; not an owner-vault consumer | Daemon needs TLS before owner login | Service-level availability; encryption-at-rest equivalence requires separate deployment review |
| Contacts/profile metadata | `data_dir()/contacts.json`, profile/device-profile files | Private filesystem permissions, generally not owner-vault encrypted | Some service/mobile paths can use it | Accepted metadata risk, not a cryptographic secret; requires product decision if contacts must be owner-private |
| Pairing grants and trusted-peer records | `~/.yandi/paired_*.json`, `trusted_peers.json`; `netlayer/pairing.rs` | Current v2 grants contain public identity/expiry data, no resume secrets; private permissions | Service/network level | PASS for current format |
| Backups | `node_backup.rs` creates signed/encrypted backup payloads and validates restore paths | Backup format is authenticated/encrypted; restore refuses unsafe paths and foreign files | Restore requires recovery/owner flow | PASS in unit coverage |

The service-level exceptions are intentional for autonomous delivery and networking.
They are not evidence that the owner chat history or owner file contents are available
after reboot.

## Access-control paths

- `ClosedEnv::start_service` unlocks only consumers whose `is_service()` is true.
- `ChatConsumer` is not a service consumer; its history is created locked and gets a
  key only through owner unlock.
- `FilesConsumer`, `ProxyConsumer`, reactions, local overrides, and the bootstrap
  signer are owner-level consumers unless explicitly marked otherwise in
  `closed_api.rs`.
- `SelfConsumer` exposes only the public contact bundle while locked; private signing
  and encryption keys are revoked.
- `mobile_api.rs` queues incoming E2E blobs for the PC while locked, but does not
  decrypt them into chat history.
- `web/ui_socket.rs` checks `SO_PEERCRED`, UID equality, directory ownership/modes,
  socket ownership/mode, hard links, symlinks, and a lock file before accepting UI
  requests. Local socket capability does not bypass admin StepUp checks.
- `web/server.rs` applies authentication, admin guards, and locked-content guards.
  The `/api/chat/*` coverage was added in this audit.

## Damage and recovery behavior

`EnvelopeStore`, `SealedJson`, group storage, chat storage, file storage, and the
mobile-self migration paths preserve damaged/foreign files rather than replacing them
with an empty state. `ClosedEnv::open` treats a damaged envelope as configured and
records an error; it does not silently start a new owner environment. The identity
loader also refuses to replace an existing identity that cannot be opened.

The recovery path is deliberately destructive only after the configured threshold of
wrong recovery phrases, or after explicit panic erase. Panic erase revokes in-memory
keys first, checks removal/fsync results, reports leftovers, and does not erase the
node identity or pairing backups.

## Confirmed defect and fix

### Fixed: chat routes outside the locked-content guard

Before the fix, `is_owner_content_path` covered `/api/groups` and file content but not
`/api/chat`. `ChatStorage::load_history` still failed closed because its key mode was
`Locked`, so no plaintext disclosure was reproduced. The route policy was nevertheless
incomplete and future handlers could accidentally bypass the intended boundary.

Fix: include `/api/chat` in `src/web/server.rs` and add exact-prefix regression tests.

Status: **FIXED**, defense-in-depth; protocol and storage formats unchanged.

## Risks and limitations, not confirmed vulnerabilities

1. Contacts, profiles, pairing metadata, TLS material, and device-service metadata are
   not all owner-vault encrypted. Their availability is part of the service-level
   design, but the privacy classification should be confirmed by product requirements.
2. A privileged local root process can read daemon memory or files. The design boundary
   is protection from ordinary users, accidental Desktop access, and disk theft without
   the required keys; it is not protection from a privileged host administrator while
   the daemon is running.
3. Previous on-disk writes may leave recoverable remnants on SSDs. `ClosedEnv` documents
   this limitation; filesystem encryption is required for that threat model.
4. The non-Unix implementation has a weaker install-key fallback in `file_vault.rs` and
   does not implement the Linux closed-environment lifecycle equivalently. Windows was
   not part of this Linux audit and is **NOT TESTED**.

## Tests and verification

Available static/unit checks:

- Rust library suite before this documentation-only audit plus the route fix:
  `914 passed, 0 failed, 1 ignored`.
- Existing closed-environment tests cover setup, wrong password, recovery, lock,
  restart, migration, concurrent lock/unlock, and service consumers.
- Existing tests cover damaged sealed JSON, damaged group state, damaged identity,
  encrypted file containers, UI socket UID/path checks, and backup restore safety.
- After the route fix: run the targeted `locked_guard_tests` and a full Rust library
  suite before merging.
- `git diff --check` remains required.

## NOT TESTED

- Clean Debian 12 installation with a running systemd PID 1 and a fresh user profile.
- Clean Ubuntu 24.04 amd64 installation.
- Reboot persistence of the automatically selected `yandi-node@<user>.service`.
- Full CLI-only setup/unlock/lock exercise against an installed `.deb`.
- Desktop installed after CLI setup and connected to the same existing vault.
- Physical GUI login/admin flow and real network/mobile traffic during this audit.
- Windows closed-environment equivalence.

## Safe physical follow-up

Use a disposable user/profile or VM. Install `yandi-node` first, run the CLI setup,
record only the synthetic test identity, restart the service, and verify:

```text
systemctl status yandi-node@<user>.service
journalctl -u yandi-node@<user>.service -b
yandi setup --help
```

Then install Desktop and verify that it discovers the existing Unix socket and vault.
After locking, confirm that chat history, groups, and file content return a locked
response while service delivery retains only the intended ciphertext/count behavior.
