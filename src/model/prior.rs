use super::{Expert, Prediction, Residual};
use crate::features::{self, Features};

const ANCHORS: [(f64, f64); 3] = [(1.0, 0.35), (2.477, 0.70), (4.0, 0.95)];
const CONTENT_SLOPE: f64 = -0.10;
const MIN_P: f64 = 0.05;
pub const VARIANCE: f64 = 0.04;

pub fn curve(log_lux: f64) -> f64 {
    let l = log_lux.clamp(0.0, 4.5);
    let [(x0, y0), (x1, y1), (x2, y2)] = ANCHORS;
    y0 * (l - x1) * (l - x2) / ((x0 - x1) * (x0 - x2))
        + y1 * (l - x0) * (l - x2) / ((x1 - x0) * (x1 - x2))
        + y2 * (l - x0) * (l - x1) / ((x2 - x0) * (x2 - x1))
}

pub fn mean(x: &Features) -> f64 {
    let log_lux = features::feature_to_log_lux(x[features::LUX]);
    let content = CONTENT_SLOPE * (x[features::SCREEN_LUMA] - 0.5);
    (curve(log_lux) + content).clamp(MIN_P, 1.0)
}

#[derive(Debug, Default, Clone)]
pub struct Prior;

impl Expert for Prior {
    fn predict(&self, _x: &Features) -> Prediction {
        Prediction {
            mean: 0.0,
            variance: VARIANCE,
        }
    }

    fn fit(&mut self, _data: &[Residual]) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::DIM;

    #[test]
    fn passes_through_anchors() {
        for (l, p) in ANCHORS {
            assert!((curve(l) - p).abs() < 1e-12);
        }
    }

    #[test]
    fn rises_with_light_and_is_concave() {
        let values: Vec<f64> = (0..=45).map(|i| curve(i as f64 / 10.0)).collect();
        assert!(values.windows(2).all(|w| w[1] > w[0]));
        let slopes: Vec<f64> = values.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(slopes.windows(2).all(|w| w[1] < w[0]));
    }

    #[test]
    fn evening_room_matches_comfort_study() {
        let level = |p: f64| crate::perceptual::to_level(p, 100);
        let evening = curve(30f64.log10());
        assert!(
            (8..=20).contains(&level(evening)),
            "30 lx gives level {}",
            level(evening)
        );
    }

    #[test]
    fn bright_content_lowers_target() {
        let mut dark = [0.0; DIM];
        dark[features::SCREEN_LUMA] = 0.1;
        let mut bright = dark;
        bright[features::SCREEN_LUMA] = 0.9;
        assert!(mean(&bright) < mean(&dark));
    }
}
