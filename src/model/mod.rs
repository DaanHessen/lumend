pub mod ensemble;
mod linalg;
pub mod linear;
pub mod mlp;
pub mod neighbors;
pub mod prior;
pub mod rng;
pub mod short_term;

use crate::features::Features;
use serde::{Deserialize, Serialize};

const RECENCY_HALF_LIFE_S: f64 = 90.0 * 86_400.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    pub mean: f64,
    pub variance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub time: f64,
    pub x: Vec<f64>,
    pub p: f64,
    pub weight: f64,
}

impl Sample {
    pub fn new(time: f64, x: &Features, p: f64, weight: f64) -> Self {
        Self {
            time,
            x: x.to_vec(),
            p,
            weight,
        }
    }

    pub fn features(&self) -> Option<Features> {
        self.x.as_slice().try_into().ok()
    }

    pub fn is_weak(&self) -> bool {
        self.weight < 1.0
    }
}

pub trait Expert {
    fn predict(&self, x: &Features) -> Prediction;
    fn fit(&mut self, data: &[Residual]);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residual {
    pub x: Features,
    pub residual: f64,
    pub weight: f64,
}

pub fn residuals(samples: &[Sample]) -> Vec<Residual> {
    let newest = samples.iter().map(|s| s.time).fold(f64::MIN, f64::max);
    samples
        .iter()
        .filter_map(|s| {
            let x = s.features()?;
            let age = (newest - s.time).max(0.0);
            Some(Residual {
                x,
                residual: s.p - prior::mean(&x),
                weight: s.weight * 0.5f64.powf(age / RECENCY_HALF_LIFE_S),
            })
        })
        .filter(|r| r.weight > 1e-6 && r.residual.is_finite())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::DIM;

    #[test]
    fn residuals_decay_with_age_and_skip_bad_rows() {
        let x = [0.0; DIM];
        let samples = vec![
            Sample::new(0.0, &x, 0.5, 1.0),
            Sample::new(RECENCY_HALF_LIFE_S, &x, 0.5, 1.0),
            Sample {
                time: 0.0,
                x: vec![0.0; 3],
                p: 0.5,
                weight: 1.0,
            },
        ];
        let r = residuals(&samples);
        assert_eq!(r.len(), 2);
        assert!((r[0].weight - 0.5).abs() < 1e-12);
        assert_eq!(r[1].weight, 1.0);
    }
}
