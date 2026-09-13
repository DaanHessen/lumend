#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.uniform()
    }

    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.uniform() * n as f64) as usize % n.max(1)
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_in_range_and_roughly_flat() {
        let mut rng = Rng::new(7);
        let values: Vec<f64> = (0..10_000).map(|_| rng.uniform()).collect();
        assert!(values.iter().all(|v| (0.0..1.0).contains(v)));
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        assert!((mean - 0.5).abs() < 0.02);
    }

    #[test]
    fn normal_has_unit_variance() {
        let mut rng = Rng::new(3);
        let values: Vec<f64> = (0..20_000).map(|_| rng.normal()).collect();
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
        assert!(mean.abs() < 0.03 && (var - 1.0).abs() < 0.05);
    }

    #[test]
    fn same_seed_same_sequence() {
        let a: Vec<u64> = (0..5)
            .scan(Rng::new(9), |r, _| Some(r.next_u64()))
            .collect();
        let b: Vec<u64> = (0..5)
            .scan(Rng::new(9), |r, _| Some(r.next_u64()))
            .collect();
        assert_eq!(a, b);
    }
}
