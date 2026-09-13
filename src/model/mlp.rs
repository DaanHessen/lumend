use super::rng::Rng;
use super::{Expert, Prediction, Residual};
use crate::features::{DIM, Features};
use serde::{Deserialize, Serialize};

const H1: usize = 16;
const H2: usize = 16;
const W1: usize = 0;
const B1: usize = W1 + H1 * DIM;
const W2: usize = B1 + H1;
const B2: usize = W2 + H2 * H1;
const W3: usize = B2 + H2;
const B3: usize = W3 + H2;
const PARAMS: usize = B3 + 1;

const LEARNING_RATE: f64 = 0.005;
const WEIGHT_DECAY: f64 = 1e-3;
const BATCH: usize = 32;
const SAMPLE_PASSES: usize = 20_000;
const MAX_EPOCHS: usize = 400;
const UNTRAINED_VARIANCE: f64 = 0.05;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mlp {
    params: Vec<f64>,
    train_mse: f64,
    trained_on: usize,
    seed: u64,
}

struct Activations {
    h1: [f64; H1],
    h2: [f64; H2],
    out: f64,
}

impl Mlp {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let mut params = vec![0.0; PARAMS];
        let mut init = |range: std::ops::Range<usize>, fan_in: usize, fan_out: usize| {
            let limit = (6.0 / (fan_in + fan_out) as f64).sqrt();
            for p in &mut params[range] {
                *p = rng.range(-limit, limit);
            }
        };
        init(W1..B1, DIM, H1);
        init(W2..B2, H1, H2);
        Self {
            params,
            train_mse: 0.0,
            trained_on: 0,
            seed,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.params.len() == PARAMS && self.params.iter().all(|p| p.is_finite())
    }

    fn forward(&self, x: &Features) -> Activations {
        let p = &self.params;
        let mut h1 = [0.0; H1];
        for (i, h) in h1.iter_mut().enumerate() {
            let row = &p[W1 + i * DIM..W1 + (i + 1) * DIM];
            *h = (p[B1 + i] + row.iter().zip(x).map(|(w, v)| w * v).sum::<f64>()).tanh();
        }
        let mut h2 = [0.0; H2];
        for (i, h) in h2.iter_mut().enumerate() {
            let row = &p[W2 + i * H1..W2 + (i + 1) * H1];
            *h = (p[B2 + i] + row.iter().zip(&h1).map(|(w, v)| w * v).sum::<f64>()).tanh();
        }
        let out = p[B3] + p[W3..B3].iter().zip(&h2).map(|(w, v)| w * v).sum::<f64>();
        Activations { h1, h2, out }
    }

    fn accumulate_gradient(&self, sample: &Residual, grad: &mut [f64]) {
        let p = &self.params;
        let a = self.forward(&sample.x);
        let d_out = 2.0 * sample.weight * (a.out - sample.residual);

        grad[B3] += d_out;
        let mut dz2 = [0.0; H2];
        for i in 0..H2 {
            grad[W3 + i] += d_out * a.h2[i];
            dz2[i] = d_out * p[W3 + i] * (1.0 - a.h2[i] * a.h2[i]);
        }

        let mut dh1 = [0.0; H1];
        for i in 0..H2 {
            grad[B2 + i] += dz2[i];
            for j in 0..H1 {
                grad[W2 + i * H1 + j] += dz2[i] * a.h1[j];
                dh1[j] += p[W2 + i * H1 + j] * dz2[i];
            }
        }

        for j in 0..H1 {
            let dz1 = dh1[j] * (1.0 - a.h1[j] * a.h1[j]);
            grad[B1 + j] += dz1;
            for k in 0..DIM {
                grad[W1 + j * DIM + k] += dz1 * sample.x[k];
            }
        }
    }

    fn weighted_mse(&self, data: &[Residual]) -> f64 {
        let total: f64 = data.iter().map(|r| r.weight).sum();
        if total <= 0.0 {
            return 0.0;
        }
        data.iter()
            .map(|r| r.weight * (self.forward(&r.x).out - r.residual).powi(2))
            .sum::<f64>()
            / total
    }
}

impl Expert for Mlp {
    fn predict(&self, x: &Features) -> Prediction {
        let confidence = 20.0 / (20.0 + self.trained_on as f64);
        Prediction {
            mean: self.forward(x).out,
            variance: self.train_mse + UNTRAINED_VARIANCE * confidence,
        }
    }

    /// Minibatch AdamW, warm-started from the current weights so that a new
    /// correction nudges the network instead of replacing it. The number of
    /// epochs shrinks as the data grows so every fit costs about the same.
    fn fit(&mut self, data: &[Residual]) {
        if data.is_empty() {
            return;
        }
        let epochs = (SAMPLE_PASSES / data.len()).clamp(4, MAX_EPOCHS);
        let mut rng = Rng::new(self.seed ^ data.len() as u64);
        let mut order: Vec<usize> = (0..data.len()).collect();
        let mut m = vec![0.0; PARAMS];
        let mut v = vec![0.0; PARAMS];
        let mut grad = vec![0.0; PARAMS];
        let (beta1, beta2, eps) = (0.9f64, 0.999f64, 1e-8);
        let mut step = 0i32;

        for _ in 0..epochs {
            rng.shuffle(&mut order);
            for batch in order.chunks(BATCH) {
                grad.iter_mut().for_each(|g| *g = 0.0);
                let weight: f64 = batch.iter().map(|&i| data[i].weight).sum();
                for &i in batch {
                    self.accumulate_gradient(&data[i], &mut grad);
                }
                step += 1;
                let scale = 1.0 / weight.max(1e-9);
                let bc1 = 1.0 - beta1.powi(step);
                let bc2 = 1.0 - beta2.powi(step);
                for k in 0..PARAMS {
                    let g = grad[k] * scale;
                    m[k] = beta1 * m[k] + (1.0 - beta1) * g;
                    v[k] = beta2 * v[k] + (1.0 - beta2) * g * g;
                    let update = (m[k] / bc1) / ((v[k] / bc2).sqrt() + eps);
                    self.params[k] -= LEARNING_RATE * (update + WEIGHT_DECAY * self.params[k]);
                }
            }
        }
        self.train_mse = self.weighted_mse(data);
        self.trained_on = data.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::linear::Linear;

    fn sample(x0: f64, x3: f64, residual: f64) -> Residual {
        let mut x = [0.0; DIM];
        x[0] = x0;
        x[3] = x3;
        Residual {
            x,
            residual,
            weight: 1.0,
        }
    }

    #[test]
    fn untrained_network_predicts_zero() {
        let net = Mlp::new(1);
        assert_eq!(net.predict(&[0.3; DIM]).mean, 0.0);
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let mut net = Mlp::new(5);
        for (i, p) in net.params[W3..].iter_mut().enumerate() {
            *p = 0.1 * (i as f64 - 8.0);
        }
        let s = sample(0.4, 0.7, 0.2);
        let mut grad = vec![0.0; PARAMS];
        net.accumulate_gradient(&s, &mut grad);
        let loss = |n: &Mlp| s.weight * (n.forward(&s.x).out - s.residual).powi(2);
        let h = 1e-6;
        for k in [W1 + 3, W1 + 3 * DIM, B1 + 2, W2 + 17, B2 + 5, W3 + 4, B3] {
            let mut plus = net.clone();
            plus.params[k] += h;
            let mut minus = net.clone();
            minus.params[k] -= h;
            let numeric = (loss(&plus) - loss(&minus)) / (2.0 * h);
            assert!(
                (numeric - grad[k]).abs() < 1e-6,
                "param {k}: {numeric} vs {}",
                grad[k]
            );
        }
    }

    #[test]
    fn learns_an_interaction_the_linear_model_cannot() {
        let mut data = Vec::new();
        for i in 0..20 {
            for j in 0..10 {
                let x0 = -1.0 + i as f64 / 10.0;
                let x3 = j as f64 / 9.0;
                let residual = if (x0 > 0.0) == (x3 > 0.5) {
                    0.15
                } else {
                    -0.15
                };
                data.push(sample(x0, x3, residual));
            }
        }
        let mut net = Mlp::new(3);
        net.fit(&data);
        let mut linear = Linear::default();
        linear.fit(&data);
        let mse = |f: &dyn Fn(&Features) -> f64| {
            data.iter()
                .map(|r| (f(&r.x) - r.residual).powi(2))
                .sum::<f64>()
                / data.len() as f64
        };
        let net_mse = mse(&|x| net.predict(x).mean);
        let linear_mse = mse(&|x| linear.predict(x).mean);
        assert!(
            net_mse < linear_mse / 3.0,
            "mlp {net_mse} linear {linear_mse}"
        );
    }

    #[test]
    fn serialises_and_validates() {
        let net = Mlp::new(2);
        let json = serde_json::to_string(&net).unwrap();
        let back: Mlp = serde_json::from_str(&json).unwrap();
        assert_eq!(back, net);
        assert!(back.is_valid());
        let mut broken = back;
        broken.params.pop();
        assert!(!broken.is_valid());
    }
}
