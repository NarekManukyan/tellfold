//! CPython's `random.Random`, bit for bit: the Mersenne Twister, seeding from a string (`version=2`), `random()`,
//! `getrandbits`, `shuffle` and `choices`. The engine seeds it with strings like `"shoal:active:{seed}:{round}"`, so
//! the same seed must pick the same agents as the Python engine did.

use sha2::{Digest, Sha512};

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

pub struct PyRandom {
    mt: [u32; N],
    index: usize,
}

impl PyRandom {
    /// `random.Random(text)`: the text's UTF-8 bytes followed by their SHA-512, read as one big-endian integer.
    pub fn from_str_seed(text: &str) -> PyRandom {
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(&Sha512::digest(text.as_bytes()));
        PyRandom::from_big_endian(&bytes)
    }

    /// Seeds with the integer whose big-endian bytes are `bytes` (CPython splits it into 32-bit words, least
    /// significant first).
    fn from_big_endian(bytes: &[u8]) -> PyRandom {
        let mut key: Vec<u32> =
            bytes.rchunks(4).map(|chunk| chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b))).collect();
        while key.len() > 1 && key.last() == Some(&0) {
            key.pop();
        }
        if key.is_empty() {
            key.push(0);
        }
        let mut rng = PyRandom { mt: [0; N], index: N };
        rng.init_by_array(&key);
        rng
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32.wrapping_mul(prev ^ (prev >> 30)).wrapping_add(i as u32);
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
    }

    fn genrand(&mut self) -> u32 {
        if self.index >= N {
            for k in 0..N {
                let y = (self.mt[k] & UPPER_MASK) | (self.mt[(k + 1) % N] & LOWER_MASK);
                let mag = if y & 1 == 1 { MATRIX_A } else { 0 };
                self.mt[k] = self.mt[(k + M) % N] ^ (y >> 1) ^ mag;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `random()`: a float in [0, 1) with 53 random bits.
    pub fn random(&mut self) -> f64 {
        let a = self.genrand() >> 5;
        let b = self.genrand() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for k <= 64.
    pub fn getrandbits(&mut self, k: u32) -> u64 {
        match k {
            0 => 0,
            1..=32 => u64::from(self.genrand() >> (32 - k)),
            33..=64 => {
                let low = u64::from(self.genrand());
                let high = u64::from(self.genrand() >> (64 - k));
                (high << 32) | low
            }
            _ => panic!("getrandbits supports up to 64 bits"),
        }
    }

    /// `_randbelow(n)` for n > 0.
    pub fn randbelow(&mut self, n: u64) -> u64 {
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// `shuffle(items)` in place.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.randbelow(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }

    /// `choices(population, weights=weights, k=1)[0]`: the index picked. None when the weights do not sum to a
    /// positive finite number (Python raises ValueError).
    pub fn choice_weighted(&mut self, weights: &[f64]) -> Option<usize> {
        let mut cum = Vec::with_capacity(weights.len());
        let mut total = 0.0;
        for w in weights {
            total += w;
            cum.push(total);
        }
        let total = *cum.last()? + 0.0;
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        let x = self.random() * total;
        // bisect_right(cum, x, 0, n - 1)
        let hi = weights.len() - 1;
        Some(cum[..hi].partition_point(|&c| c <= x))
    }
}
