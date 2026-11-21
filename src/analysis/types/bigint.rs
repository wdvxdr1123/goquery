//! Bounded 512-bit signed two's-complement integer for exact Go constants.
use std::cmp::Ordering;
use std::fmt;

/// Fixed-width 512-bit signed two's-complement integer (little-endian limbs).
/// Any operation whose result exceeds 512 signed bits returns `None` (`Unknown`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ConstInt(pub(crate) [u64; 8]);

impl ConstInt {
    pub(crate) const ZERO: Self = Self([0; 8]);

    pub(crate) const fn from_i128(value: i128) -> Self {
        let bits = value as u128;
        let fill = if value < 0 { u64::MAX } else { 0 };
        Self([
            bits as u64,
            (bits >> 64) as u64,
            fill,
            fill,
            fill,
            fill,
            fill,
            fill,
        ])
    }

    pub(crate) const fn to_i128(self) -> Option<i128> {
        let fill = if (self.0[1] >> 63) != 0 { u64::MAX } else { 0 };
        let mut i = 2;
        while i < 8 {
            if self.0[i] != fill {
                return None;
            }
            i += 1;
        }
        let bits = (self.0[0] as u128) | ((self.0[1] as u128) << 64);
        Some(bits as i128)
    }

    pub(crate) const fn from_limbs3(limbs: [u64; 3]) -> Self {
        let fill = if (limbs[2] >> 63) != 0 { u64::MAX } else { 0 };
        Self([limbs[0], limbs[1], limbs[2], fill, fill, fill, fill, fill])
    }

    pub(crate) const fn to_limbs3(self) -> Option<[u64; 3]> {
        let fill = if (self.0[2] >> 63) != 0 { u64::MAX } else { 0 };
        let mut i = 3;
        while i < 8 {
            if self.0[i] != fill {
                return None;
            }
            i += 1;
        }
        Some([self.0[0], self.0[1], self.0[2]])
    }

    pub(crate) const fn is_zero(self) -> bool {
        let mut i = 0;
        while i < 8 {
            if self.0[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    pub(crate) const fn is_negative(self) -> bool {
        (self.0[7] >> 63) != 0
    }

    pub(crate) fn bitnot(self) -> Self {
        let mut out = [0u64; 8];
        for (dst, src) in out.iter_mut().zip(self.0) {
            *dst = !src;
        }
        Self(out)
    }

    pub(crate) fn bitand(self, rhs: Self) -> Self {
        let mut out = [0u64; 8];
        for (dst, (&a, &b)) in out.iter_mut().zip(self.0.iter().zip(&rhs.0)) {
            *dst = a & b;
        }
        Self(out)
    }

    pub(crate) fn bitor(self, rhs: Self) -> Self {
        let mut out = [0u64; 8];
        for (dst, (&a, &b)) in out.iter_mut().zip(self.0.iter().zip(&rhs.0)) {
            *dst = a | b;
        }
        Self(out)
    }

    pub(crate) fn bitxor(self, rhs: Self) -> Self {
        let mut out = [0u64; 8];
        for (dst, (&a, &b)) in out.iter_mut().zip(self.0.iter().zip(&rhs.0)) {
            *dst = a ^ b;
        }
        Self(out)
    }

    pub(crate) fn checked_neg(self) -> Option<Self> {
        let mut out = [0u64; 8];
        let mut carry = 1u128;
        for (dst, &src) in out.iter_mut().zip(&self.0) {
            let sum = (!src as u128) + carry;
            *dst = sum as u64;
            carry = sum >> 64;
        }
        let res = Self(out);
        if self.is_negative() && res.is_negative() {
            return None;
        }
        Some(res)
    }

    fn wrapping_abs(self) -> ([u64; 8], bool) {
        if !self.is_negative() {
            return (self.0, false);
        }
        let mut out = [0u64; 8];
        let mut carry = 1u128;
        for (dst, &src) in out.iter_mut().zip(&self.0) {
            let sum = (!src as u128) + carry;
            *dst = sum as u64;
            carry = sum >> 64;
        }
        (out, true)
    }

    fn from_unsigned(mag: [u64; 8], neg: bool) -> Option<Self> {
        let top = mag[7] >> 63;
        if !neg {
            return (top == 0).then_some(Self(mag));
        }
        // Only -2^511 has top == 1 with all lower bits zero.
        let is_min = top == 1 && mag[7] == (1u64 << 63) && mag[..7].iter().all(|&x| x == 0);
        if top != 0 && !is_min {
            return None;
        }
        let mut out = [0u64; 8];
        let mut carry = 1u128;
        for (dst, &src) in out.iter_mut().zip(&mag) {
            let sum = (!src as u128) + carry;
            *dst = sum as u64;
            carry = sum >> 64;
        }
        Some(Self(out))
    }

    pub(crate) fn checked_add(self, rhs: Self) -> Option<Self> {
        let mut out = [0u64; 8];
        let mut carry = 0u128;
        for (dst, (&a, &b)) in out.iter_mut().zip(self.0.iter().zip(&rhs.0)) {
            let sum = (a as u128) + (b as u128) + carry;
            *dst = sum as u64;
            carry = sum >> 64;
        }
        let res = Self(out);
        let sa = self.is_negative();
        let sb = rhs.is_negative();
        if sa == sb && res.is_negative() != sa {
            return None;
        }
        Some(res)
    }

    pub(crate) fn checked_sub(self, rhs: Self) -> Option<Self> {
        let neg = rhs.checked_neg()?;
        self.checked_add(neg)
    }

    pub(crate) fn checked_mul(self, rhs: Self) -> Option<Self> {
        let (a, neg_a) = self.wrapping_abs();
        let (b, neg_b) = rhs.wrapping_abs();
        let mut prod = [0u128; 16];
        for i in 0..8 {
            if a[i] == 0 {
                continue;
            }
            for j in 0..8 {
                let p = (a[i] as u128) * (b[j] as u128);
                prod[i + j] += p & 0xFFFF_FFFF_FFFF_FFFF;
                prod[i + j + 1] += p >> 64;
            }
        }
        let mut carry = 0u128;
        let mut full = [0u64; 16];
        for i in 0..16 {
            let sum = prod[i] + carry;
            full[i] = sum as u64;
            carry = sum >> 64;
        }
        if carry != 0 || full[8..].iter().any(|&x| x != 0) {
            return None;
        }
        let mut mag = [0u64; 8];
        mag.copy_from_slice(&full[..8]);
        Self::from_unsigned(mag, neg_a ^ neg_b)
    }

    pub(crate) fn checked_div_rem(self, rhs: Self) -> Option<(Self, Self)> {
        if rhs.is_zero() {
            return None;
        }
        let (num, neg_a) = self.wrapping_abs();
        let (den, neg_b) = rhs.wrapping_abs();
        let (q_mag, r_mag) = div_rem_u512(num, den);
        let q = Self::from_unsigned(q_mag, neg_a ^ neg_b)?;
        let r = Self::from_unsigned(r_mag, neg_a)?;
        Some((q, r))
    }

    pub(crate) fn checked_shl(self, count: u32) -> Option<Self> {
        if self.is_zero() || count == 0 {
            return Some(self);
        }
        if count >= 512 {
            return None;
        }
        let words = (count / 64) as usize;
        let bits = count % 64;
        let mut out = [0u64; 8];
        for (i, dst) in out.iter_mut().enumerate().skip(words) {
            let src = i - words;
            let mut v = self.0[src] << bits;
            if bits != 0 && src > 0 {
                v |= self.0[src - 1] >> (64 - bits);
            }
            *dst = v;
        }
        let shifted = Self(out);
        (shifted.checked_shr(count) == Some(self)).then_some(shifted)
    }

    pub(crate) fn checked_shr(self, count: u32) -> Option<Self> {
        let fill = if self.is_negative() { u64::MAX } else { 0 };
        if count >= 512 {
            return Some(Self([fill; 8]));
        }
        let words = (count / 64) as usize;
        let bits = count % 64;
        let mut out = [fill; 8];
        for (i, dst) in out.iter_mut().enumerate().take(8 - words) {
            let src = i + words;
            let mut v = self.0[src] >> bits;
            if bits != 0 {
                let next = if src + 1 < 8 { self.0[src + 1] } else { fill };
                v |= next << (64 - bits);
            }
            *dst = v;
        }
        Some(Self(out))
    }

    pub(crate) fn to_f64(self) -> f64 {
        let (mag, neg) = self.wrapping_abs();
        let mut acc = 0.0f64;
        for &limb in mag.iter().rev() {
            acc = acc * 18446744073709551616.0 + (limb as f64);
        }
        if neg { -acc } else { acc }
    }

    pub(crate) fn to_f32(self) -> Option<f32> {
        let val = self.to_f64() as f32;
        val.is_finite().then_some(val)
    }
}

impl Ord for ConstInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        for i in (0..8).rev() {
            match self.0[i].cmp(&other.0[i]) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for ConstInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ConstInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("0");
        }
        if let Some(small) = self.to_i128() {
            return write!(f, "{small}");
        }
        let (mut mag, neg) = self.wrapping_abs();
        const CHUNK: u64 = 10_000_000_000_000_000_000;
        let mut chunks = Vec::new();
        while mag.iter().any(|&x| x != 0) {
            let mut rem = 0u128;
            for limb in mag.iter_mut().rev() {
                let cur = (rem << 64) | (*limb as u128);
                *limb = (cur / (CHUNK as u128)) as u64;
                rem = cur % (CHUNK as u128);
            }
            chunks.push(rem as u64);
        }
        if neg {
            f.write_str("-")?;
        }
        let last = chunks.pop().unwrap_or(0);
        write!(f, "{last}")?;
        for chunk in chunks.into_iter().rev() {
            write!(f, "{chunk:019}")?;
        }
        Ok(())
    }
}

fn cmp_u512(a: [u64; 8], b: [u64; 8]) -> Ordering {
    for i in (0..8).rev() {
        match a[i].cmp(&b[i]) {
            Ordering::Equal => {}
            ord => return ord,
        }
    }
    Ordering::Equal
}

fn sub_u512(a: [u64; 8], b: [u64; 8]) -> [u64; 8] {
    let mut out = [0u64; 8];
    let mut borrow = 0u128;
    for i in 0..8 {
        let av = a[i] as u128;
        let bv = (b[i] as u128) + borrow;
        if av >= bv {
            out[i] = (av - bv) as u64;
            borrow = 0;
        } else {
            out[i] = ((1u128 << 64) + av - bv) as u64;
            borrow = 1;
        }
    }
    out
}

fn div_rem_u512(num: [u64; 8], den: [u64; 8]) -> ([u64; 8], [u64; 8]) {
    let mut q = [0u64; 8];
    let mut r = [0u64; 8];
    for bit in (0..512).rev() {
        let mut carry = (num[bit / 64] >> (bit % 64)) & 1;
        for limb in &mut r {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        if cmp_u512(r, den) != Ordering::Less {
            r = sub_u512(r, den);
            q[bit / 64] |= 1u64 << (bit % 64);
        }
    }
    (q, r)
}
