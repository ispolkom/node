# Files for https://yandi.su (binding invitation links to the application)

These files are used for Android App Links: `https://yandi.su/i#…` should open YANDI without a chooser.
**Status:** the website files are deployed; the application side (an `intent-filter` with `autoVerify` in
`AndroidManifest.xml` and the link handler) has not been implemented yet (stage G4 in ROADMAP11/12), so the
link currently opens in a browser.

Place these files in the website root (`/var/www/yandi_su_usr/data/www/yandi.su/`):
- `.well-known/assetlinks.json` — package `com.yandi.yandi_mobile` and the SHA-256 release-signing fingerprint (update it if the signing key changes);
- `i/index.html` — a static page for people who do not have the application (no scripts, counters, or third-party resources).

Check with `curl -s https://yandi.su/.well-known/assetlinks.json`; it must return this JSON as `application/json` without redirects.

## Website source (since 2026-10-20)
`deploy/site/yandi.su/` contains the website source without `update/` (packages and APKs are not stored in git).
The live mirror including `update/` is `/mnt/backup/node-release/site/`.
Release versions and checksums shown on download pages are in `includes/releases.php` (update it for every release).
Signed release manifests and the entry-node list are copied to `release/manifests/` (public files; no secrets).
