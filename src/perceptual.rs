const K: f64 = 30.0;

pub fn to_p(level: u32, max: u32) -> f64 {
    if max == 0 {
        return 0.0;
    }
    let f = (level.min(max) as f64) / max as f64;
    (1.0 + K * f).ln() / (1.0 + K).ln()
}

pub fn to_level(p: f64, max: u32) -> u32 {
    let p = p.clamp(0.0, 1.0);
    let f = ((1.0 + K).powf(p) - 1.0) / K;
    (f * max as f64).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(to_p(0, 100), 0.0);
        assert!((to_p(100, 100) - 1.0).abs() < 1e-12);
        assert_eq!(to_level(0.0, 100), 0);
        assert_eq!(to_level(1.0, 100), 100);
    }

    #[test]
    fn round_trip_is_exact_on_every_level() {
        for max in [100, 255, 1023, 96000] {
            for level in (0..=max).step_by((max as usize / 100).max(1)) {
                assert_eq!(
                    to_level(to_p(level, max), max),
                    level,
                    "max {max} level {level}"
                );
            }
        }
    }

    #[test]
    fn monotonic_and_expands_low_end() {
        let ps: Vec<f64> = (0..=100).map(|l| to_p(l, 100)).collect();
        assert!(ps.windows(2).all(|w| w[1] > w[0]));
        assert!(
            ps[10] > 0.3,
            "the bottom tenth of levels should cover a third of the scale"
        );
    }

    #[test]
    fn out_of_range_input_is_clamped() {
        assert_eq!(to_level(1.7, 100), 100);
        assert_eq!(to_level(-0.2, 100), 0);
        assert!((to_p(500, 100) - 1.0).abs() < 1e-12);
        assert_eq!(to_p(5, 0), 0.0);
    }
}
