//! Writes a legacy key directory (auth.json v1 + identity v2) for manual and terminal tests: make_legacy <dir> <machine-id>
use key_root::identity_store::{legacy_passphrase, seal_identity_legacy};
use key_root::legacy::seal_legacy_auth;
use key_root::*;
use zeroize::Zeroizing;
fn main() {
    let mut a = std::env::args().skip(1);
    let dir = KeyDir::open(a.next().unwrap()).unwrap();
    let machine = FixedMachine(a.next().unwrap());
    let id = IdentityMaterial {
        address: [0x11; 32],
        public_key: [0x22; 32],
        signing_public_key: [0x33; 32],
        created_at: "2026-06-28T16:14:00Z".into(),
        private_key: Zeroizing::new([0x44; 32]),
        signing_private_key: Zeroizing::new([0x55; 32]),
    };
    key_root::atomic::create_new_file(
        &dir.auth_file(),
        &seal_legacy_auth(&[7u8; 32], "$login", &machine).unwrap(),
    )
    .unwrap();
    key_root::atomic::create_new_file(
        &dir.identity_file(9000),
        &seal_identity_legacy(&id, &legacy_passphrase(None, &machine, &id.address)).unwrap(),
    )
    .unwrap();
}
