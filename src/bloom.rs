//! Fixed-storage Bloom filters for conservative candidate rejection.
//! A negative probe excludes a byte key; a positive probe still needs exact
//! validation. Filters own only bits, never source keys or heap allocations.

/// A reusable, deterministic probe seed for one byte key.
/// Equal fingerprints do not prove that the original byte keys were equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint(u64);

impl Fingerprint {
    /// Hash a byte key once for probes into multiple filter sizes.
    pub fn new(key: &[u8]) -> Self {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in key {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        Self(hash)
    }
}

/// Three probes into a power-of-two number of 64-bit words.
/// The default filter occupies 256 bytes; `BloomFilter<1>` occupies eight.
#[derive(Clone, Debug)]
pub struct BloomFilter<const WORDS: usize = 32> {
    words: [u64; WORDS],
}

impl<const WORDS: usize> Default for BloomFilter<WORDS> {
    fn default() -> Self {
        Self::new()
    }
}

// The full filter contract is exercised by unit tests in this module even when
// production callers only need construction, insertion and fingerprint probes.
#[allow(dead_code)]
impl<const WORDS: usize> BloomFilter<WORDS> {
    /// Construct an empty filter.
    ///
    /// # Panics
    /// Panics unless WORDS is a nonzero power of two and its bit count fits usize.
    pub fn new() -> Self {
        assert!(
            WORDS.is_power_of_two() && WORDS <= usize::MAX / 64,
            "Bloom storage must have a nonzero power-of-two word count"
        );
        Self { words: [0; WORDS] }
    }

    /// Add a byte key without retaining its bytes.
    pub fn insert(&mut self, key: &[u8]) {
        self.insert_fingerprint(Fingerprint::new(key));
    }

    /// Add a previously computed key fingerprint.
    pub fn insert_fingerprint(&mut self, fingerprint: Fingerprint) {
        for bit in Self::probes(fingerprint) {
            self.words[bit / 64] |= 1 << (bit % 64);
        }
    }

    /// Return false only when this filter definitely does not contain the key.
    pub fn may_contain(&self, key: &[u8]) -> bool {
        self.may_contain_fingerprint(Fingerprint::new(key))
    }

    /// Probe a seed computed once and reused across many independent filters.
    pub fn may_contain_fingerprint(&self, fingerprint: Fingerprint) -> bool {
        Self::probes(fingerprint)
            .into_iter()
            .all(|bit| self.words[bit / 64] & (1 << (bit % 64)) != 0)
    }

    /// Add all keys represented by another filter of the same configuration.
    pub fn union(&mut self, other: &Self) {
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    /// Reset to an empty set. Individual keys cannot be deleted safely.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    /// Whether no bits are set, including after a reset.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    fn probes(fingerprint: Fingerprint) -> [usize; 3] {
        let mask = WORDS * 64 - 1;
        let start = fingerprint.0 as usize & mask;
        let step = ((fingerprint.0 >> 32) as usize | 1) & mask;
        [
            start,
            start.wrapping_add(step) & mask,
            start.wrapping_add(step.wrapping_mul(2)) & mask,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{BloomFilter, Fingerprint};

    /// Membership contracts across configurations, collisions and resets.
    #[test]
    fn filter_contract() {
        fn check<const WORDS: usize>() {
            let mut left = BloomFilter::<WORDS>::new();
            let mut right = BloomFilter::<WORDS>::new();
            assert!(left.is_empty());
            assert!(!left.may_contain(b"absent"));
            let keys: Vec<Vec<u8>> = (0..5000u32)
                .map(|i| i.to_le_bytes().to_vec())
                .chain([Vec::new(), "变量".as_bytes().to_vec()])
                .collect();
            for (i, key) in keys.iter().enumerate() {
                if i % 2 == 0 {
                    left.insert(key);
                } else {
                    right.insert_fingerprint(Fingerprint::new(key));
                }
            }
            left.union(&right);
            for key in &keys {
                assert!(left.may_contain(key));
                assert!(left.may_contain_fingerprint(Fingerprint::new(key)));
            }
            left.clear();
            assert!(left.is_empty());
            for key in &keys {
                assert!(!left.may_contain(key));
            }
        }
        check::<1>();
        check::<4>();
        check::<32>();
        assert_eq!(std::mem::size_of::<BloomFilter>(), 256);
        assert_eq!(std::mem::size_of::<BloomFilter<1>>(), 8);
        let mut filter = BloomFilter::<32>::new();
        for i in 0..100 {
            filter.insert(format!("present_{i}").as_bytes());
        }
        let false_positives = (0..10000)
            .filter(|i| filter.may_contain(format!("absent_{i}").as_bytes()))
            .count();
        // Broad deterministic bound catches a useless filter without forbidding collisions.
        assert!(false_positives < 1000, "false positives: {false_positives}");
        let mut other = BloomFilter::<32>::new();
        for i in 0..100 {
            other.insert(format!("second_{i}").as_bytes());
        }
        filter.union(&other);
        for i in 0..100 {
            assert!(filter.may_contain(format!("present_{i}").as_bytes()));
            assert!(filter.may_contain(format!("second_{i}").as_bytes()));
        }
    }
}
