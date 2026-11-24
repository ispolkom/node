//! The seed phrase: what the owner writes down at the first start.
//!
//! 12, 18 or 24 English words of the BIP-39 list encode 128, 192 or 256 random bits plus a checksum (24 is the default the apps offer). The words
//! are only how the secret is written: the key is made from the *bytes* (`secret()`), never from the text, so the display language can never
//! change it. A phrase that fails its own check (wrong count, a word not on the list, a typo) is reported as a typo before any slow key
//! derivation runs, and the caller must not count it as a guess.
//!
//! An optional **control word** (one word the owner invents and keeps apart from the paper) is mixed into the secret with Argon2id. Without
//! it the same words give another node. It is carried inside the phrase string after [`CONTROL_SEP`] so that every place that takes "the
//! phrase" takes the word too; the apps never show the separator.
use crate::wrap::random_bytes;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

const LIST: &str = include_str!("bip39_en.txt");

/// Internal separator between the words and the control word inside one string (U+001F is not white space, so it never splits a word).
pub const CONTROL_SEP: char = '\u{1f}';
pub const CONTROL_WORD_MIN: usize = 8;
pub const CONTROL_WORD_MAX: usize = 64;

fn list() -> Vec<&'static str> {
    LIST.lines().map(str::trim).filter(|w| !w.is_empty()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedError {
    /// Not 12, 18 or 24 words.
    WrongCount(usize),
    /// The word at this position (from 1) is not in the list.
    UnknownWord(usize),
    /// All words are on the list but the phrase does not pass its checksum (a typo or swapped words).
    Checksum,
    /// The control word has spaces: it is ONE word.
    ControlWordSpaces,
    /// The control word is shorter than [`CONTROL_WORD_MIN`] characters.
    ControlWordShort,
    /// The control word is longer than [`CONTROL_WORD_MAX`] characters.
    ControlWordLong,
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeedError::WrongCount(n) => write!(f, "Нужно 12, 18 или 24 слова, введено {n}"),
            SeedError::UnknownWord(i) => write!(f, "Слово №{i} не из списка"),
            SeedError::Checksum => write!(f, "Фраза не проходит проверку: опечатка или слова переставлены"),
            SeedError::ControlWordSpaces => write!(f, "Контрольное слово — одно слово, без пробелов"),
            SeedError::ControlWordShort => write!(f, "Контрольное слово не короче {CONTROL_WORD_MIN} знаков"),
            SeedError::ControlWordLong => write!(f, "Контрольное слово не длиннее {CONTROL_WORD_MAX} знаков"),
        }
    }
}

/// The random secret behind the words (16, 24 or 32 bytes for 12, 18 or 24 words) and the optional control word.
pub struct Seed {
    entropy: Zeroizing<Vec<u8>>,
    control: Option<Zeroizing<String>>,
}

fn checksum_bits(entropy: &[u8]) -> u8 {
    Sha256::digest(entropy)[0]
}

/// The control word as it is used: trimmed, one word, Unicode-normalised (the same letters typed on another keyboard or system give the
/// same key), and within the length limits.
pub fn normalize_control_word(word: &str) -> Result<String, SeedError> {
    let w = word.trim();
    if w.chars().any(char::is_whitespace) || w.contains(CONTROL_SEP) {
        return Err(SeedError::ControlWordSpaces);
    }
    let w: String = w.nfc().collect();
    let n = w.chars().count();
    if n < CONTROL_WORD_MIN {
        return Err(SeedError::ControlWordShort);
    }
    if n > CONTROL_WORD_MAX {
        return Err(SeedError::ControlWordLong);
    }
    Ok(w)
}

/// The words and the control word as ONE phrase string for every API that takes "the phrase". An empty word means none.
pub fn join_phrase(words: &str, control_word: &str) -> String {
    if control_word.trim().is_empty() {
        words.to_owned()
    } else {
        format!("{words}{CONTROL_SEP}{control_word}")
    }
}

/// Argon2id cost of mixing in the control word: about a second per guess, 128 MiB per guess for whoever stole the paper and is guessing.
/// (memory KiB, passes, lanes). Changing it changes every node that has a control word: never edit it, only add a new version.
const PRODUCTION_WORD_KDF: (u32, u32, u32) = (131072, 3, 1);
#[cfg(not(test))]
const WORD_KDF: (u32, u32, u32) = PRODUCTION_WORD_KDF;
/// The unit tests of everything else use a cheap cost (the real one is pinned by its own test with a known answer).
#[cfg(test)]
const WORD_KDF: (u32, u32, u32) = (64, 1, 1);

fn word_key(entropy: &[u8], word: &str) -> Zeroizing<[u8; 32]> {
    word_key_with(entropy, word, WORD_KDF)
}

fn word_key_with(entropy: &[u8], word: &str, cost: (u32, u32, u32)) -> Zeroizing<[u8; 32]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let mut password = Zeroizing::new(Vec::with_capacity(2 + word.len() + entropy.len()));
    password.extend_from_slice(&(word.len() as u16).to_be_bytes());
    password.extend_from_slice(word.as_bytes());
    password.extend_from_slice(entropy);
    let salt = Sha256::digest(b"yandi/seed-word/v2");
    let params = Params::new(cost.0, cost.1, cost.2, Some(32)).expect("fixed valid parameters");
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(&password, &salt, out.as_mut_slice())
        .expect("fixed valid parameters and a 32-byte salt");
    out
}

impl Seed {
    fn bare(entropy: Vec<u8>) -> Seed {
        Seed { entropy: Zeroizing::new(entropy), control: None }
    }

    /// A new random seed of 12, 18 or 24 words.
    pub fn generate(words: usize) -> Option<Seed> {
        match words {
            12 => Some(Seed::bare(random_bytes::<16>().to_vec())),
            18 => Some(Seed::bare(random_bytes::<24>().to_vec())),
            24 => Some(Seed::bare(random_bytes::<32>().to_vec())),
            _ => None,
        }
    }

    pub fn from_entropy(bytes: &[u8]) -> Option<Seed> {
        matches!(bytes.len(), 16 | 24 | 32).then(|| Seed::bare(bytes.to_vec()))
    }

    /// The same seed with a control word (see [`normalize_control_word`]). An empty word leaves the seed as it is.
    pub fn with_control_word(mut self, word: &str) -> Result<Seed, SeedError> {
        self.control = if word.trim().is_empty() { None } else { Some(Zeroizing::new(normalize_control_word(word)?)) };
        Ok(self)
    }

    pub fn has_control_word(&self) -> bool {
        self.control.is_some()
    }

    pub fn word_count(&self) -> usize {
        self.entropy.len() * 3 / 4
    }

    /// The words, in order, as they are shown and written down. The control word is never part of them.
    pub fn words(&self) -> Vec<&'static str> {
        let words = list();
        let cs_len = self.entropy.len() / 4; // checksum bits: 4 for 128, 6 for 192, 8 for 256
        let mut bits = Zeroizing::new(Vec::<bool>::with_capacity(self.entropy.len() * 8 + 8));
        for b in self.entropy.iter() {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1 == 1);
            }
        }
        let cs = checksum_bits(&self.entropy);
        for i in 0..cs_len {
            bits.push((cs >> (7 - i)) & 1 == 1);
        }
        bits.chunks(11)
            .map(|c| words[c.iter().fold(0usize, |a, b| (a << 1) | *b as usize)])
            .collect()
    }

    /// Parse what the person typed: words separated by any whitespace, any letter case; optionally followed by [`CONTROL_SEP`] and the
    /// control word.
    pub fn parse(input: &str) -> Result<Seed, SeedError> {
        let (words_part, control_part) = match input.split_once(CONTROL_SEP) {
            Some((a, b)) => (a, Some(b)),
            None => (input, None),
        };
        let typed: Vec<String> = words_part.split_whitespace().map(|w| w.to_lowercase()).collect();
        if !matches!(typed.len(), 12 | 18 | 24) {
            return Err(SeedError::WrongCount(typed.len()));
        }
        let words = list();
        let mut bits = Zeroizing::new(Vec::<bool>::with_capacity(264));
        for (i, w) in typed.iter().enumerate() {
            let idx = words.iter().position(|x| x == w).ok_or(SeedError::UnknownWord(i + 1))?;
            for k in (0..11).rev() {
                bits.push((idx >> k) & 1 == 1);
            }
        }
        let ent_bits = typed.len() * 11 * 32 / 33; // 128, 192 or 256
        let mut entropy = Zeroizing::new(vec![0u8; ent_bits / 8]);
        for (i, b) in bits[..ent_bits].iter().enumerate() {
            if *b {
                entropy[i / 8] |= 1 << (7 - i % 8);
            }
        }
        let cs = checksum_bits(&entropy);
        let cs_len = ent_bits / 32;
        let ok = (0..cs_len).all(|i| bits[ent_bits + i] == ((cs >> (7 - i)) & 1 == 1));
        if !ok {
            return Err(SeedError::Checksum);
        }
        let seed = Seed { entropy, control: None };
        match control_part {
            None | Some("") => Ok(seed),
            Some(w) => seed.with_control_word(w),
        }
    }

    /// What the keys are made from. Language-independent: only the bytes, with a version and a purpose in front. With a control word the
    /// bytes are first stretched together with it (Argon2id), so the words alone no longer lead to the same keys.
    pub fn secret(&self) -> Zeroizing<String> {
        match &self.control {
            None => {
                let hex: String = self.entropy.iter().map(|b| format!("{b:02x}")).collect();
                Zeroizing::new(format!("yandi-seed-v1:{hex}"))
            }
            Some(word) => {
                let key = word_key(&self.entropy, word);
                Zeroizing::new(format!("yandi-seed-v2:{}", hex::encode(*key)))
            }
        }
    }

    /// The node's root key, derived from the secret: the same words (and control word) give the same root on any machine.
    pub fn root(&self) -> Zeroizing<[u8; 32]> {
        crate::wrap::root_from_seed_secret(&self.secret())
    }

    pub fn same_as(&self, other: &Seed) -> bool {
        let eq = self.entropy.len() == other.entropy.len()
            && self.entropy.iter().zip(other.entropy.iter()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        eq && self.control.as_deref().map(String::as_str) == other.control.as_deref().map(String::as_str)
    }
}

/// True if the bytes are the secret of a seed phrase ("yandi-seed-v1:…" or "yandi-seed-v2:…"), as opposed to a typed password.
pub fn looks_like_seed_secret(secret: &[u8]) -> bool {
    secret.starts_with(b"yandi-seed-v1:") || secret.starts_with(b"yandi-seed-v2:")
}

/// True if the text is made only of words from the list and has 12, 18 or 24 of them (even with a typo in the checksum): so a mistake is
/// reported as a mistake in a seed phrase, and not as a wrong password. A control word after the separator is not counted.
pub fn looks_like_seed(input: &str) -> bool {
    let words_part = input.split_once(CONTROL_SEP).map_or(input, |(a, _)| a);
    let n = words_part.split_whitespace().count();
    matches!(n, 12 | 18 | 24) && !matches!(Seed::parse(words_part), Err(SeedError::UnknownWord(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entropy(byte: u8, n: usize) -> Seed {
        Seed::from_entropy(&vec![byte; n]).unwrap()
    }

    #[test]
    fn the_list_is_the_standard_one() {
        let l = list();
        assert_eq!(l.len(), 2048);
        assert_eq!(l[0], "abandon");
        assert_eq!(l[2047], "zoo");
        let mut sorted = l.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 2048);
        // the first four letters tell every word apart (the standard guarantees it)
        let mut prefixes: Vec<_> = l.iter().map(|w| w.chars().take(4).collect::<String>()).collect();
        prefixes.sort();
        prefixes.dedup();
        assert_eq!(prefixes.len(), 2048);
    }

    #[test]
    fn standard_test_vectors() {
        assert_eq!(entropy(0x00, 16).words().join(" "), "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
        assert_eq!(entropy(0x7f, 16).words().join(" "), "legal winner thank year wave sausage worth useful legal winner thank yellow");
        assert_eq!(
            entropy(0x00, 32).words().join(" "),
            format!("{}art", "abandon ".repeat(23))
        );
        assert_eq!(entropy(0xff, 16).words().join(" "), "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong");
    }

    #[test]
    fn generated_seeds_round_trip_in_all_three_sizes() {
        for n in [12usize, 18, 24] {
            let s = Seed::generate(n).unwrap();
            assert_eq!(s.words().len(), n);
            assert_eq!(s.word_count(), n);
            let back = Seed::parse(&s.words().join(" ")).unwrap();
            assert!(s.same_as(&back));
            assert_eq!(*s.secret(), *back.secret());
        }
        assert!(Seed::generate(13).is_none());
        let (a, b) = (Seed::generate(12).unwrap(), Seed::generate(12).unwrap());
        assert!(!a.same_as(&b));
    }

    #[test]
    fn input_is_forgiving_about_spacing_and_case_but_not_about_words() {
        let s = entropy(0x7f, 16);
        let messy = "  LEGAL winner\tthank\nyear wave sausage worth useful legal winner thank   Yellow ";
        assert!(s.same_as(&Seed::parse(messy).unwrap()));
        assert_eq!(Seed::parse("legal winner").err(), Some(SeedError::WrongCount(2)));
        assert_eq!(Seed::parse("").err(), Some(SeedError::WrongCount(0)));
        let bad_word = "legal winner thank year wave sausage worth useful legal winner thank yelow";
        assert_eq!(Seed::parse(bad_word).err(), Some(SeedError::UnknownWord(12)));
        // a valid word in the wrong place: all on the list, checksum fails
        let swapped = "winner legal thank year wave sausage worth useful legal winner thank yellow";
        assert_eq!(Seed::parse(swapped).err(), Some(SeedError::Checksum));
        let last_wrong = "legal winner thank year wave sausage worth useful legal winner thank zoo";
        assert_eq!(Seed::parse(last_wrong).err(), Some(SeedError::Checksum));
    }

    #[test]
    fn the_secret_is_the_bytes_and_a_label() {
        assert_eq!(&*entropy(0x00, 16).secret(), "yandi-seed-v1:00000000000000000000000000000000");
        assert_ne!(*entropy(0x00, 16).secret(), *entropy(0x00, 32).secret());
    }

    #[test]
    fn looks_like_a_seed_even_with_a_checksum_typo_but_not_with_foreign_words() {
        assert!(looks_like_seed("legal winner thank year wave sausage worth useful legal winner thank yellow"));
        assert!(looks_like_seed("winner legal thank year wave sausage worth useful legal winner thank yellow"));
        assert!(!looks_like_seed("legal winner thank year wave sausage worth useful legal winner thank yelow"));
        assert!(!looks_like_seed("one two three"));
        assert!(!looks_like_seed("correct horse battery staple correct horse battery staple correct horse battery staple"));
    }

    #[test]
    fn recovery_secret_takes_the_phrase_by_its_bytes() {
        let words = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let s = entropy(0x7f, 16);
        assert_eq!(*crate::recovery_secret(words), *s.secret());
        assert_eq!(*crate::recovery_secret(&words.to_uppercase().replace(' ', "   ")), *s.secret());
        // a phrase with a typo is just typed text: it cannot open anything
        assert_eq!(*crate::recovery_secret("winner legal thank year wave sausage worth useful legal winner thank yellow"), "winner legal thank year wave sausage worth useful legal winner thank yellow");
    }

    const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

    #[test]
    fn eighteen_words_use_a_192_bit_seed_and_the_standard_checksum() {
        let s = entropy(0x7f, 24);
        assert_eq!(s.word_count(), 18);
        let w = s.words();
        assert_eq!(w.len(), 18);
        assert!(s.same_as(&Seed::parse(&w.join(" ")).unwrap()));
        // a typo in the 18-word checksum is a checksum error, not a count error
        let mut bad = w.clone();
        bad.swap(0, 1);
        if bad != w {
            assert!(matches!(Seed::parse(&bad.join(" ")), Err(SeedError::Checksum) | Ok(_)));
        }
        assert_eq!(Seed::parse(&w[..17].join(" ")).err(), Some(SeedError::WrongCount(17)));
    }

    #[test]
    fn without_a_control_word_the_secret_is_the_old_one_and_the_root_is_fixed() {
        let s = Seed::parse(PHRASE).unwrap();
        assert!(!s.has_control_word());
        assert_eq!(&*s.secret(), "yandi-seed-v1:7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f");
        assert_eq!(*s.root(), *Seed::parse(PHRASE).unwrap().root(), "same words, same root");
        assert_ne!(*s.root(), *Seed::parse(&entropy(0x00, 16).words().join(" ")).unwrap().root());
    }

    #[test]
    fn the_control_word_changes_every_key_and_is_part_of_the_phrase_string() {
        let plain = Seed::parse(PHRASE).unwrap();
        let with = Seed::parse(&join_phrase(PHRASE, "Воробей-2026")).unwrap();
        let other = Seed::parse(&join_phrase(PHRASE, "Воробей-2027")).unwrap();
        let again = Seed::parse(&join_phrase(PHRASE, "Воробей-2026")).unwrap();
        assert!(with.has_control_word());
        assert!(with.secret().starts_with("yandi-seed-v2:"));
        assert_eq!(*with.secret(), *again.secret(), "the same word gives the same keys");
        assert_ne!(*with.secret(), *plain.secret(), "without the word it is another node");
        assert_ne!(*with.secret(), *other.secret());
        assert_ne!(*with.root(), *plain.root());
        assert_ne!(*with.root(), *other.root());
        assert!(with.same_as(&again) && !with.same_as(&plain) && !with.same_as(&other));
        // the words themselves never show the control word
        assert_eq!(with.words(), plain.words());
        // the same word with another seed gives unrelated keys (the seed is part of the stretching)
        let seed2 = entropy(0x11, 16).words().join(" ");
        let other_seed = Seed::parse(&join_phrase(&seed2, "Воробей-2026")).unwrap();
        assert_ne!(*with.secret(), *other_seed.secret());
        // an empty word means none
        assert!(!Seed::parse(&join_phrase(PHRASE, "   ")).unwrap().has_control_word());
        assert_eq!(*Seed::parse(&format!("{PHRASE}{CONTROL_SEP}")).unwrap().secret(), *plain.secret());
    }

    #[test]
    fn the_control_word_is_one_word_of_a_sensible_length_and_unicode_normalised() {
        let base = || Seed::parse(PHRASE).unwrap();
        assert_eq!(base().with_control_word("два слова вместе").err(), Some(SeedError::ControlWordSpaces));
        assert_eq!(base().with_control_word("коротко").err(), Some(SeedError::ControlWordShort));
        assert_eq!(base().with_control_word(&"я".repeat(65)).err(), Some(SeedError::ControlWordLong));
        assert!(base().with_control_word(&"я".repeat(64)).is_ok());
        // letters are case sensitive
        let a = base().with_control_word("Воробей-2026").unwrap();
        let b = base().with_control_word("воробей-2026").unwrap();
        assert_ne!(*a.secret(), *b.secret());
        // «й» typed as one letter or as «и» + a combining breve is the same word
        let composed = "Мой\u{439}Ключик1";
        let decomposed = "Мой\u{438}\u{306}Ключик1";
        assert_ne!(composed, decomposed);
        assert_eq!(*base().with_control_word(composed).unwrap().secret(), *base().with_control_word(decomposed).unwrap().secret());
        // surrounding spaces are not part of the word
        assert_eq!(*base().with_control_word("  Воробей-2026 ").unwrap().secret(), *a.secret());
    }

    #[test]
    fn a_phrase_with_a_control_word_still_looks_like_a_seed_and_a_bad_word_is_reported_as_such() {
        assert!(looks_like_seed(&join_phrase(PHRASE, "Воробей-2026")));
        assert_eq!(Seed::parse(&join_phrase(PHRASE, "два слова")).err(), Some(SeedError::ControlWordSpaces));
        assert_eq!(Seed::parse(&join_phrase(PHRASE, "коротко")).err(), Some(SeedError::ControlWordShort));
        // a mistake in the words is still a mistake in the words
        let swapped = "winner legal thank year wave sausage worth useful legal winner thank yellow";
        assert_eq!(Seed::parse(&join_phrase(swapped, "Воробей-2026")).err(), Some(SeedError::Checksum));
    }

    #[test]
    fn recovery_secret_carries_the_control_word_too() {
        let joined = join_phrase(PHRASE, "Воробей-2026");
        assert_eq!(*crate::recovery_secret(&joined), *Seed::parse(&joined).unwrap().secret());
        assert_ne!(*crate::recovery_secret(&joined), *crate::recovery_secret(PHRASE));
    }

    /// The REAL cost with a known answer: a node made today must still be restored by every later build. If this fails, the key derivation changed.
    #[test]
    fn the_production_cost_gives_the_pinned_answer() {
        assert_eq!(PRODUCTION_WORD_KDF, (131072, 3, 1));
        let key = word_key_with(&[0x7f; 16], "Воробей-2026", PRODUCTION_WORD_KDF);
        assert_eq!(hex::encode(*key), "2fb07e5d04361e255db58a378d854df017f0758d7ea778f1327db7e90a0573e2", "проверено независимо (Python argon2-cffi)");
    }
}
