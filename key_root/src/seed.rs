//! The seed phrase: what the owner writes down at the first start.
//!
//! 12 (default) or 24 English words of the BIP-39 list encode 128 or 256 random bits plus a checksum. The words are only how the
//! secret is written: the key is made from the *bytes* (`secret()`), never from the text, so the display language can never change it.
//! A phrase that fails its own check (wrong count, a word not on the list, a typo) is reported as a typo before any slow key
//! derivation runs, and the caller must not count it as a guess.
use crate::wrap::random_bytes;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const LIST: &str = include_str!("bip39_en.txt");

fn list() -> Vec<&'static str> {
    LIST.lines().map(str::trim).filter(|w| !w.is_empty()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedError {
    /// Not 12 or 24 words.
    WrongCount(usize),
    /// The word at this position (from 1) is not in the list.
    UnknownWord(usize),
    /// All words are on the list but the phrase does not pass its checksum (a typo or swapped words).
    Checksum,
}

impl std::fmt::Display for SeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeedError::WrongCount(n) => write!(f, "Нужно 12 или 24 слова, введено {n}"),
            SeedError::UnknownWord(i) => write!(f, "Слово №{i} не из списка"),
            SeedError::Checksum => write!(f, "Фраза не проходит проверку: опечатка или слова переставлены"),
        }
    }
}

/// The random secret behind the words (16 bytes for 12 words, 32 for 24).
pub struct Seed(Zeroizing<Vec<u8>>);

fn checksum_bits(entropy: &[u8]) -> u8 {
    Sha256::digest(entropy)[0]
}

impl Seed {
    /// A new random seed of 12 or 24 words.
    pub fn generate(words: usize) -> Option<Seed> {
        match words {
            12 => Some(Seed(Zeroizing::new(random_bytes::<16>().to_vec()))),
            24 => Some(Seed(Zeroizing::new(random_bytes::<32>().to_vec()))),
            _ => None,
        }
    }

    pub fn from_entropy(bytes: &[u8]) -> Option<Seed> {
        matches!(bytes.len(), 16 | 32).then(|| Seed(Zeroizing::new(bytes.to_vec())))
    }

    pub fn word_count(&self) -> usize {
        self.0.len() * 3 / 4
    }

    /// The words, in order, as they are shown and written down.
    pub fn words(&self) -> Vec<&'static str> {
        let words = list();
        let cs_len = self.0.len() / 4; // checksum bits: 4 for 128, 8 for 256
        let mut bits = Zeroizing::new(Vec::<bool>::with_capacity(self.0.len() * 8 + 8));
        for b in self.0.iter() {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1 == 1);
            }
        }
        let cs = checksum_bits(&self.0);
        for i in 0..cs_len {
            bits.push((cs >> (7 - i)) & 1 == 1);
        }
        bits.chunks(11)
            .map(|c| words[c.iter().fold(0usize, |a, b| (a << 1) | *b as usize)])
            .collect()
    }

    /// Parse what the person typed: words separated by any whitespace, any letter case.
    pub fn parse(input: &str) -> Result<Seed, SeedError> {
        let typed: Vec<String> = input.split_whitespace().map(|w| w.to_lowercase()).collect();
        if typed.len() != 12 && typed.len() != 24 {
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
        let ent_bits = typed.len() * 11 * 32 / 33; // 128 or 256
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
        Ok(Seed(entropy))
    }

    /// What the keys are made from. Language-independent: only the bytes, with a version and a purpose in front.
    pub fn secret(&self) -> Zeroizing<String> {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        Zeroizing::new(format!("yandi-seed-v1:{hex}"))
    }

    pub fn same_as(&self, other: &Seed) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(other.0.iter()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
    }
}

/// True if the text is made only of words from the list and has 12 or 24 of them (even with a typo in the checksum): so a mistake is
/// reported as a mistake in a seed phrase, and not as a wrong password.
pub fn looks_like_seed(input: &str) -> bool {
    let n = input.split_whitespace().count();
    (n == 12 || n == 24) && !matches!(Seed::parse(input), Err(SeedError::UnknownWord(_)))
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
    fn generated_seeds_round_trip_in_both_sizes() {
        for n in [12usize, 24] {
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
}
