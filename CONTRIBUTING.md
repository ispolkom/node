# Contributing

Thank you for helping. The rules are short.

1. **Read [`docs/INVARIANTS.md`](docs/INVARIANTS.md).** These are the project promises, such as “an exit node cannot open the owner's home network”. Change them only through a separate maintainer decision, not as a side effect of another task. `tests/invariants.rs` checks that the document and the tests agree.
2. **Security comes before new features.** Every new feature needs an attacker-focused test: forgery, replay, malformed input, or overload. The wire format is frozen by golden vectors (`docs/WIRE.md`): if the bytes change, increase the protocol version instead of changing the vector.
3. **Keep dependencies to a minimum.** Justify every new dependency; `cargo deny check` must pass in CI.
4. **Build and check before opening a merge request:**
   ```bash
   scripts/check.sh              # build + quick tests
   scripts/check.sh полностью    # + network tests on real processes (requires a public address)
   ```
5. **Run background loops through `supervisor::supervise`**, rather than a bare `tokio::spawn` so task failures are visible and handled. `scripts/check_spawn.sh` checks the number of bare spawns.
6. **Secrets and addresses.** Do not commit keys, passwords, logs, or addresses of other machines. Change the entry-node list only through `yandi bootstrap …` and the owner's signature (see [`bootstrap/README.md`](bootstrap/README.md)).
7. **One request, one change.** Describe what changed, why it changed, how it was checked, and what could break.
8. **Report vulnerabilities** through [`SECURITY.md`](SECURITY.md), not in public issues.

Code comments and documents are currently being translated to English; English is the preferred language for new technical comments and documents.
