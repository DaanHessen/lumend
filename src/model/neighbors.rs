use super::{Expert, Prediction, Residual, prior};
use crate::features::Features;

const BANDWIDTH: f64 = 0.7;
const PSEUDO_MASS: f64 = 0.5;

#[derive(Debug, Default, Clone)]
pub struct Neighbors {
    points: Vec<Residual>,
}

impl Expert for Neighbors {
    fn predict(&self, x: &Features) -> Prediction {
        let scale = -0.5 / (BANDWIDTH * BANDWIDTH);
        let kernels: Vec<(f64, f64)> = self
            .points
            .iter()
            .map(|p| {
                let d2: f64 = p.x.iter().zip(x).map(|(a, b)| (a - b) * (a - b)).sum();
                (p.weight * (scale * d2).exp(), p.residual)
            })
            .collect();
        let mass: f64 = kernels.iter().map(|(k, _)| k).sum();
        let mean = kernels.iter().map(|(k, r)| k * r).sum::<f64>() / (mass + PSEUDO_MASS);
        let spread = kernels
            .iter()
            .map(|(k, r)| k * (r - mean).powi(2))
            .sum::<f64>()
            / (mass + PSEUDO_MASS);
        Prediction {
            mean,
            variance: prior::VARIANCE / (1.0 + mass) + spread,
        }
    }

    fn fit(&mut self, data: &[Residual]) {
        self.points = data.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::DIM;

    fn point(v: f64, residual: f64) -> Residual {
        Residual {
            x: [v; DIM],
            residual,
            weight: 1.0,
        }
    }

    #[test]
    fn empty_model_defers_to_prior() {
        let p = Neighbors::default().predict(&[0.0; DIM]);
        assert_eq!(p.mean, 0.0);
        assert_eq!(p.variance, prior::VARIANCE);
    }

    #[test]
    fn repeated_situation_is_recalled() {
        let mut model = Neighbors::default();
        model.fit(&vec![point(0.2, 0.12); 20]);
        let p = model.predict(&[0.2; DIM]);
        assert!((p.mean - 0.12).abs() < 0.005);
        assert!(p.variance < 0.003);
    }

    #[test]
    fn far_from_data_falls_back_to_prior() {
        let mut model = Neighbors::default();
        model.fit(&[point(0.0, 0.2)]);
        let p = model.predict(&[1.0; DIM]);
        assert!(p.mean.abs() < 1e-6);
        assert!((p.variance - prior::VARIANCE).abs() < 1e-6);
    }
}
