//! Bounded stream reassembly; no retransmission, and no bytes beyond a gap.
use super::rx_pipeline::{env_limit, Limits};
use std::{collections::BTreeMap, time::Duration};
use tokio::time::Instant;

pub(crate) struct Cursor {
    next: u64,
    ordered: bool,
}
impl Cursor {
    pub fn new(ordered: bool) -> Self {
        Self { next: 0, ordered }
    }
    pub fn block(&mut self, len: usize) -> Result<Option<u64>, ()> {
        let start = self.next;
        self.next = self
            .next
            .checked_add(u64::try_from(len).map_err(|_| ())?)
            .ok_or(())?;
        Ok(self.ordered.then_some(start))
    }
    pub fn end(&self) -> Option<u64> {
        self.ordered.then_some(self.next)
    }
}

pub(crate) trait Chunk {
    fn offset(&self) -> Option<u64>;
    fn len(&self) -> usize;
    fn close(&self) -> bool;
}
#[derive(Debug, PartialEq)]
pub(crate) enum Error {
    Overflow,
    Invalid,
    Gap,
}

pub(crate) struct Reorder<T> {
    pub next: u64,
    pending: BTreeMap<u64, (T, bool)>,
    pub bytes: usize,
    final_offset: Option<u64>,
    gap_since: Option<Instant>,
    limits: Limits,
    deadline: Duration,
}
impl<T: Chunk> Reorder<T> {
    pub fn configured() -> Self {
        let queue = Limits::tunnel();
        Self::new(
            Limits {
                items: env_limit("YANDI_TUNNEL_REORDER_ITEMS", queue.items, 65536).min(queue.items),
                bytes: env_limit("YANDI_TUNNEL_REORDER_BYTES", queue.bytes, 256 * 1024 * 1024)
                    .min(queue.bytes),
            },
            Duration::from_millis(env_limit("YANDI_TUNNEL_GAP_TIMEOUT_MS", 5000, 600000) as u64),
        )
    }
    pub fn new(limits: Limits, deadline: Duration) -> Self {
        Self {
            next: 0,
            pending: BTreeMap::new(),
            bytes: 0,
            final_offset: None,
            gap_since: None,
            limits,
            deadline,
        }
    }
    pub fn size(&self) -> usize {
        self.pending.len()
    }
    pub fn expires(&self) -> Option<Instant> {
        self.gap_since.map(|t| t + self.deadline)
    }
    pub fn check(&self, now: Instant) -> Result<(), Error> {
        if self.expires().is_some_and(|t| now >= t) {
            Err(Error::Gap)
        } else {
            Ok(())
        }
    }
    // Returns (reordered, duplicate). Close is stored separately, never overtakes data.
    pub fn insert(&mut self, block: T, now: Instant) -> Result<(bool, bool), Error> {
        self.check(now)?;
        let start = block.offset().ok_or(Error::Invalid)?;
        let end = start
            .checked_add(u64::try_from(block.len()).map_err(|_| Error::Invalid)?)
            .ok_or(Error::Invalid)?;
        if block.close() {
            if block.len() != 0
                || start < self.next
                || self.final_offset.is_some_and(|f| f != start)
                || self
                    .pending
                    .iter()
                    .any(|(&o, b)| o.checked_add(b.0.len() as u64).is_none_or(|e| e > start))
            {
                return Err(Error::Invalid);
            }
            let duplicate = self.final_offset == Some(start);
            self.final_offset = Some(start);
            self.update_gap(now);
            return Ok((start > self.next, duplicate));
        }
        if self.final_offset.is_some_and(|f| end > f) {
            return Err(Error::Invalid);
        }
        if end <= self.next || block.len() == 0 {
            return Ok((false, true));
        }
        if let Some(old) = self.pending.get(&start) {
            // An equal start with a longer suffix must not silently lose new bytes.
            if old.0.len() >= block.len() {
                return Ok((false, true));
            }
        }
        let old_len = self.pending.get(&start).map_or(0, |b| b.0.len());
        let bytes = self
            .bytes
            .checked_sub(old_len)
            .and_then(|v| v.checked_add(block.len()))
            .ok_or(Error::Overflow)?;
        if (old_len == 0 && self.pending.len() >= self.limits.items) || bytes > self.limits.bytes {
            return Err(Error::Overflow);
        }
        let duplicate = start < self.next || old_len > 0;
        let reordered = start > self.next;
        self.bytes = bytes;
        self.pending.insert(start, (block, duplicate));
        self.update_gap(now);
        Ok((reordered, duplicate))
    }
    pub fn pop(&mut self, now: Instant) -> Option<(T, usize, bool)> {
        let (&start, _) = self.pending.first_key_value()?;
        if start > self.next {
            return None;
        }
        let (block, counted_duplicate) = self.pending.remove(&start).unwrap();
        self.bytes -= block.len();
        let skip = (self.next - start).min(block.len() as u64) as usize;
        if skip < block.len() {
            self.next = start + block.len() as u64; // checked at admission
            self.gap_since = None; // progress starts a new gap deadline
        }
        self.update_gap(now);
        Some((block, skip, counted_duplicate))
    }
    pub fn finished(&self) -> bool {
        self.final_offset == Some(self.next) && self.pending.is_empty()
    }
    fn update_gap(&mut self, now: Instant) {
        let gap = self
            .pending
            .first_key_value()
            .is_some_and(|(&s, _)| s > self.next)
            || (self.pending.is_empty() && self.final_offset.is_some_and(|f| f > self.next));
        if gap {
            self.gap_since.get_or_insert(now);
        } else {
            self.gap_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)]
    struct B(u64, Vec<u8>, bool);
    impl Chunk for B {
        fn offset(&self) -> Option<u64> {
            Some(self.0)
        }
        fn len(&self) -> usize {
            self.1.len()
        }
        fn close(&self) -> bool {
            self.2
        }
    }
    fn order() -> Reorder<B> {
        Reorder::new(
            Limits {
                items: 4,
                bytes: 16,
            },
            Duration::from_millis(50),
        )
    }
    fn drain(r: &mut Reorder<B>, now: Instant) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some((b, skip, _)) = r.pop(now) {
            out.extend_from_slice(&b.1[skip..]);
        }
        out
    }
    #[test]
    fn reverse_duplicates_overlap_and_final_close() {
        let now = Instant::now();
        let mut r = order();
        r.insert(B(6, vec![], true), now).unwrap();
        assert!(!r.finished());
        assert_eq!(
            r.insert(B(3, b"def".to_vec(), false), now),
            Ok((true, false))
        );
        assert_eq!(
            r.insert(B(3, b"def".to_vec(), false), now),
            Ok((false, true))
        );
        assert!(drain(&mut r, now).is_empty());
        r.insert(B(0, b"abcd".to_vec(), false), now).unwrap();
        assert_eq!(drain(&mut r, now), b"abcdef");
        assert!(r.finished());
        assert_eq!((r.size(), r.bytes), (0, 0));
        assert_eq!(
            r.insert(B(0, b"abcdef".to_vec(), false), now),
            Ok((false, true))
        );
    }
    #[test]
    fn delivered_prefix_trimmed_and_same_start_longer_preserved() {
        let now = Instant::now();
        let mut r = order();
        r.insert(B(0, b"abc".to_vec(), false), now).unwrap();
        assert_eq!(drain(&mut r, now), b"abc");
        r.insert(B(1, b"bcde".to_vec(), false), now).unwrap();
        assert_eq!(drain(&mut r, now), b"de");
        r.insert(B(6, b"g".to_vec(), false), now).unwrap();
        r.insert(B(6, b"ghi".to_vec(), false), now).unwrap();
        r.insert(B(5, b"f".to_vec(), false), now).unwrap();
        assert_eq!(drain(&mut r, now), b"fghi");
    }
    #[test]
    fn gap_deadline_cannot_be_extended_by_later_blocks_or_duplicates() {
        let now = Instant::now();
        let mut r = order();
        r.insert(B(2, vec![2], false), now).unwrap();
        r.insert(B(4, vec![4], false), now + Duration::from_millis(40))
            .unwrap();
        r.insert(B(2, vec![2], false), now + Duration::from_millis(45))
            .unwrap();
        assert_eq!(r.check(now + Duration::from_millis(50)), Err(Error::Gap));
        assert!(drain(&mut r, now).is_empty());
    }
    #[test]
    fn progress_starts_deadline_for_the_next_gap_and_idle_has_no_deadline() {
        let now = Instant::now();
        let mut r = order();
        assert_eq!(r.expires(), None);
        r.insert(B(2, vec![2], false), now).unwrap();
        r.insert(B(5, vec![5], false), now).unwrap();
        r.insert(B(0, vec![0, 1], false), now + Duration::from_millis(40))
            .unwrap();
        assert_eq!(
            drain(&mut r, now + Duration::from_millis(40)),
            vec![0, 1, 2]
        );
        assert_eq!(r.expires(), Some(now + Duration::from_millis(90)));
    }
    #[test]
    fn byte_and_item_overflow() {
        let now = Instant::now();
        let mut r = order();
        assert_eq!(
            r.insert(B(1, vec![0; 17], false), now),
            Err(Error::Overflow)
        );
        for o in 1..=4 {
            r.insert(B(o, vec![0], false), now).unwrap();
        }
        assert_eq!(r.insert(B(5, vec![0], false), now), Err(Error::Overflow));
        assert_eq!((r.size(), r.bytes), (4, 4));
    }
    #[test]
    fn close_gap_and_inconsistent_final_offsets() {
        let now = Instant::now();
        let mut r = order();
        r.insert(B(4, vec![], true), now).unwrap();
        assert_eq!(r.check(now + Duration::from_millis(50)), Err(Error::Gap));
        let mut r = order();
        r.insert(B(2, vec![2, 3], false), now).unwrap();
        assert_eq!(r.insert(B(3, vec![], true), now), Err(Error::Invalid));
        r.insert(B(4, vec![], true), now).unwrap();
        assert_eq!(r.insert(B(5, vec![], true), now), Err(Error::Invalid));
        assert_eq!(r.insert(B(4, vec![4], false), now), Err(Error::Invalid));
        assert_eq!(r.insert(B(0, vec![0], true), now), Err(Error::Invalid));
    }
    #[test]
    fn u64_exhaustion_never_wraps() {
        let now = Instant::now();
        let mut r = order();
        r.next = u64::MAX - 1;
        assert_eq!(
            r.insert(B(u64::MAX, vec![0], false), now),
            Err(Error::Invalid)
        );
        r.insert(B(u64::MAX - 1, vec![7], false), now).unwrap();
        assert_eq!(drain(&mut r, now), vec![7]);
        r.insert(B(u64::MAX, vec![], true), now).unwrap();
        assert!(r.finished());
        let mut c = Cursor::new(true);
        c.next = u64::MAX - 1;
        assert_eq!(c.block(1), Ok(Some(u64::MAX - 1)));
        assert_eq!(c.block(1), Err(()));
        assert_eq!(c.end(), Some(u64::MAX));
    }
}
