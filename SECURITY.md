# Security

## Reporting a vulnerability

**Do not publish vulnerabilities in public issues or discussions.** Contact the repository owners privately through *Security → Report a vulnerability* on GitHub. Include what you found, how to reproduce it, the affected version or commit, and what you think could happen.

We will respond as soon as possible, fix the issue, and describe the fix publicly later if we agree with you. Do not use a finding against other people's running nodes.

## What counts as a vulnerability

Examples include reading or altering another user's traffic; bypassing signature or replay checks; reaching the owner's home network through an exit node; executing attacker-controlled data as code; crashing or hanging a node with attacker-controlled bytes; leaking keys or passwords into logs or files; and replacing the entry-node list (`bootstrap/`).

## What the project promises and what it does not

The promises guarded by tests are listed in [`docs/INVARIANTS.md`](docs/INVARIANTS.md). Known limitations and the plan for addressing them are in [`docs/CRYPTO_MAP.md`](docs/CRYPTO_MAP.md) and [`docs/ROADMAP.md`](docs/ROADMAP.md). In short:

- the project **has not undergone an independent audit**;
- encryption is applied per link between neighboring nodes, not end-to-end from person to person (the end-to-end layer is not complete);
- the network **must not be considered anonymous** against an observer who can see a significant part of it;
- an exit node is visible as the source of traffic leaving through it, so the exit owner is responsible for traffic sent through that node.

## For node operators

- Keep the node up to date. Do not open the exit to “everyone” unless you are prepared to take responsibility for other people's traffic; the default is trusted nodes only.
- Secrets such as keys, the login password, device secrets, and the entry-list signing key are created with mode `0600`. Do not expose them or put them into unprotected backups.
- The administration page listens only on `127.0.0.1`; do not expose it externally.
