const HALF_LIFE_S: f64 = 600.0;
const RESET_LUX_RATIO: f64 = 3.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShortTerm {
    offset: Option<Offset>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Offset {
    value: f64,
    set_at: f64,
    lux: f64,
}

impl ShortTerm {
    pub fn set(&mut self, value: f64, now: f64, lux: f64) {
        self.offset = Some(Offset {
            value,
            set_at: now,
            lux: lux.max(1.0),
        });
    }

    pub fn clear(&mut self) {
        self.offset = None;
    }

    pub fn value(&self, now: f64, lux: f64) -> f64 {
        let Some(o) = self.offset else { return 0.0 };
        let ratio = lux.max(1.0) / o.lux;
        if !(1.0 / RESET_LUX_RATIO..=RESET_LUX_RATIO).contains(&ratio) {
            return 0.0;
        }
        o.value * 0.5f64.powf((now - o.set_at).max(0.0) / HALF_LIFE_S)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_every_ten_minutes() {
        let mut s = ShortTerm::default();
        s.set(0.2, 1000.0, 100.0);
        assert_eq!(s.value(1000.0, 100.0), 0.2);
        assert!((s.value(1600.0, 100.0) - 0.1).abs() < 1e-12);
        assert!((s.value(2200.0, 100.0) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn large_light_change_resets() {
        let mut s = ShortTerm::default();
        s.set(-0.1, 0.0, 300.0);
        assert_eq!(s.value(10.0, 250.0), -0.1 * 0.5f64.powf(10.0 / HALF_LIFE_S));
        assert_eq!(s.value(10.0, 1000.0), 0.0);
        assert_eq!(s.value(10.0, 90.0), 0.0);
    }

    #[test]
    fn cleared_is_zero() {
        let mut s = ShortTerm::default();
        s.set(0.3, 0.0, 10.0);
        s.clear();
        assert_eq!(s.value(0.0, 10.0), 0.0);
    }
}
