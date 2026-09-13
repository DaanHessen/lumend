use super::{Expert, Prediction, Residual, linalg};
use crate::features::{DIM, Features};

const N: usize = DIM + 1;
const PRIOR_PRECISION: f64 = 4.0;
const NOISE_VARIANCE: f64 = 0.005;

#[derive(Debug, Clone)]
pub struct Linear {
    mean: [f64; N],
    covariance: Vec<f64>,
}

impl Default for Linear {
    fn default() -> Self {
        let mut covariance = vec![0.0; N * N];
        for i in 0..N {
            covariance[i * N + i] = 1.0 / PRIOR_PRECISION;
        }
        Self {
            mean: [0.0; N],
            covariance,
        }
    }
}

fn basis(x: &Features) -> [f64; N] {
    let mut phi = [1.0; N];
    phi[1..].copy_from_slice(x);
    phi
}

impl Linear {
    pub fn contributions(&self, x: &Features) -> [f64; DIM] {
        let mut out = [0.0; DIM];
        for (i, value) in out.iter_mut().enumerate() {
            *value = self.mean[i + 1] * x[i];
        }
        out
    }
}

impl Expert for Linear {
    fn predict(&self, x: &Features) -> Prediction {
        let phi = basis(x);
        let mean = phi.iter().zip(&self.mean).map(|(a, b)| a * b).sum();
        let mut spread = 0.0;
        for i in 0..N {
            let row: f64 = (0..N).map(|j| self.covariance[i * N + j] * phi[j]).sum();
            spread += phi[i] * row;
        }
        Prediction {
            mean,
            variance: NOISE_VARIANCE + spread.max(0.0),
        }
    }

    fn fit(&mut self, data: &[Residual]) {
        let beta = 1.0 / NOISE_VARIANCE;
        let mut precision = vec![0.0; N * N];
        let mut rhs = [0.0; N];
        for i in 0..N {
            precision[i * N + i] = PRIOR_PRECISION;
        }
        for r in data {
            let phi = basis(&r.x);
            let w = beta * r.weight;
            for i in 0..N {
                rhs[i] += w * phi[i] * r.residual;
                for j in 0..=i {
                    precision[i * N + j] += w * phi[i] * phi[j];
                }
            }
        }
        for i in 0..N {
            for j in 0..i {
                precision[j * N + i] = precision[i * N + j];
            }
        }
        let Some(covariance) = linalg::spd_inverse(&precision, N) else {
            tracing::warn!("linear model update was numerically unstable, keeping previous fit");
            return;
        };
        for i in 0..N {
            self.mean[i] = (0..N).map(|j| covariance[i * N + j] * rhs[j]).sum();
        }
        self.covariance = covariance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::rng::Rng;

    #[test]
    fn untrained_predicts_zero_residual_with_prior_uncertainty() {
        let p = Linear::default().predict(&[0.0; DIM]);
        assert_eq!(p.mean, 0.0);
        assert!((p.variance - (NOISE_VARIANCE + 1.0 / PRIOR_PRECISION)).abs() < 1e-12);
    }

    #[test]
    fn recovers_a_linear_function() {
        let mut rng = Rng::new(11);
        let truth = |x: &Features| 0.1 + 0.2 * x[0] - 0.15 * x[3];
        let data: Vec<Residual> = (0..40)
            .map(|_| {
                let mut x = [0.0; DIM];
                x[0] = rng.range(-1.0, 1.0);
                x[3] = rng.range(0.0, 1.0);
                Residual {
                    x,
                    residual: truth(&x) + 0.01 * rng.normal(),
                    weight: 1.0,
                }
            })
            .collect();
        let mut model = Linear::default();
        model.fit(&data);
        for probe in [[0.5, 0.2], [-0.8, 0.9]] {
            let mut x = [0.0; DIM];
            x[0] = probe[0];
            x[3] = probe[1];
            let p = model.predict(&x);
            assert!(
                (p.mean - truth(&x)).abs() < 0.02,
                "{} vs {}",
                p.mean,
                truth(&x)
            );
        }
    }

    #[test]
    fn uncertainty_shrinks_with_data() {
        let mut model = Linear::default();
        let before = model.predict(&[0.0; DIM]).variance;
        let data = vec![
            Residual {
                x: [0.0; DIM],
                residual: 0.1,
                weight: 1.0
            };
            10
        ];
        model.fit(&data);
        assert!(model.predict(&[0.0; DIM]).variance < before / 5.0);
    }
}
