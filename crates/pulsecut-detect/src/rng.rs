//! Tiny deterministic PRNG (SplitMix64) so synthetic datasets are reproducible across
//! platforms and crate versions.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    /// Poisson sample (Knuth for small λ, normal approximation for large λ).
    pub fn poisson(&mut self, lambda: f64) -> u32 {
        if lambda <= 0.0 {
            return 0;
        }
        if lambda > 40.0 {
            let n = self.normal() * lambda.sqrt() + lambda;
            return n.round().max(0.0) as u32;
        }
        let l = (-lambda).exp();
        let mut k = 0u32;
        let mut p = 1.0;
        loop {
            p *= self.f64();
            if p <= l {
                return k;
            }
            k += 1;
        }
    }
    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_and_poisson_mean() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut r = Rng::new(1);
        let n = 20_000;
        let mean: f64 = (0..n).map(|_| r.poisson(3.0) as f64).sum::<f64>() / n as f64;
        assert!((mean - 3.0).abs() < 0.1, "{mean}");
    }
}
