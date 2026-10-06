//! The recovery code: what the owner writes down.
//!
//! A person-chosen password typed with no echo is a poor secret for something that must never be lost: a wrong layout, a caps-lock or
//! one slipped key and it "does not fit", with nothing to say which. So the system makes the secret. It is 120 random bits in an alphabet
//! without look-alike characters, shown once as `XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-CCCC`, where the last group is a check: a typo is reported
//! as a typo ("does not pass its check"), not as a wrong code, and typing it in any case, with spaces or without dashes, works.
//!
//! The code is used exactly like a password by the recovery wrapper (Argon2id over its canonical form); nothing in the file format changes.
use crate::error::{KeyRootError, Result};
use crate::wrap::random_bytes;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Crockford base32: no I, L, O, U.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PAYLOAD_CHARS: usize = 24; // 120 bits
const CHECK_CHARS: usize = 4;
pub const CODE_CHARS: usize = PAYLOAD_CHARS + CHECK_CHARS;

pub struct RecoveryCode(Zeroizing<String>); // canonical: 28 upper-case characters, no separators

fn encode_payload(bytes: &[u8; 15]) -> String {
    let mut out = String::with_capacity(PAYLOAD_CHARS);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in bytes {
        acc = (acc << 8) | *b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    out
}

fn check_group(payload: &str) -> String {
    let digest =
        Sha256::digest([b"yandi/recovery-code/v1|".as_slice(), payload.as_bytes()].concat());
    let n = ((digest[0] as u32) << 12) | ((digest[1] as u32) << 4) | ((digest[2] as u32) >> 4); // 20 bits
    (0..CHECK_CHARS)
        .rev()
        .map(|i| ALPHABET[((n >> (5 * i)) & 31) as usize] as char)
        .collect()
}

impl RecoveryCode {
    pub fn generate() -> RecoveryCode {
        let payload = encode_payload(&random_bytes::<15>());
        let check = check_group(&payload);
        RecoveryCode(Zeroizing::new(format!("{payload}{check}")))
    }

    /// The code as shown to the owner: groups of four separated by dashes.
    pub fn display(&self) -> String {
        self.0
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// The canonical string the recovery wrapper is keyed with.
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// Read a code typed by a person: case, spaces, dashes and underscores do not matter; `O` is read as `0`, `I` and `L` as `1`.
    /// Anything that is not a well-formed code with a matching check is an error that says which kind of mistake it is.
    pub fn parse(input: &str) -> Result<RecoveryCode> {
        let mut canon = String::with_capacity(CODE_CHARS);
        for ch in input.chars() {
            if ch.is_whitespace() || ch == '-' || ch == '_' {
                continue;
            }
            let c = match ch.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
                return Err(KeyRootError::Invalid(
                    "the recovery code contains a character that cannot be part of it",
                ));
            }
            canon.push(c);
        }
        if canon.len() < CODE_CHARS {
            return Err(KeyRootError::Invalid("the recovery code is incomplete"));
        }
        if canon.len() > CODE_CHARS {
            return Err(KeyRootError::Invalid("the recovery code is too long"));
        }
        let (payload, check) = canon.split_at(PAYLOAD_CHARS);
        if check_group(payload) != check {
            return Err(KeyRootError::Invalid(
                "the recovery code does not pass its check: there is a typo in it",
            ));
        }
        Ok(RecoveryCode(Zeroizing::new(canon)))
    }

    pub fn same_as(&self, other: &RecoveryCode) -> bool {
        use subtle_eq::ct_eq;
        ct_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

/// What to key the recovery wrapper with for whatever the person typed: a well-formed recovery code is used in its canonical form;
/// anything else is taken as a password, exactly as typed (older key directories were made with a chosen password).
pub fn recovery_secret(input: &str) -> Zeroizing<String> {
    match RecoveryCode::parse(input) {
        Ok(code) => Zeroizing::new(code.secret().to_owned()),
        Err(_) => Zeroizing::new(input.to_owned()),
    }
}

/// True if the text looks like an attempt at a recovery code (so that a typo can be reported as a typo, not as "wrong").
pub fn looks_like_code(input: &str) -> bool {
    let letters = input.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    (CODE_CHARS - 6..=CODE_CHARS + 6).contains(&letters)
        && input.contains('-')
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c.is_whitespace() || c == '_')
}

mod subtle_eq {
    /// Equality without an early exit (the codes are compared once, at confirmation, but there is no reason to leak the position).
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}
