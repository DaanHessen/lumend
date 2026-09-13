use super::linear::Linear;
use super::mlp::Mlp;
use super::neighbors::Neighbors;
use super::prior::{self, Prior};
use super::{Expert, Prediction, Residual, Sample, residuals};
use crate::features::Features;
use serde::{Deserialize, Serialize};

pub const EXPERTS: [&str; 4] = ["prior", "linear", "mlp", "neighbors"];
const INITIAL_WEIGHTS: [f64; 4] = [0.85, 0.05, 0.05, 0.05];
const LEARNING_RATE: f64 = 20.0;
const WEIGHT_FLOOR: f64 = 0.01;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnsembleState {
    pub weights: [f64; 4],
    pub mlp: Mlp,
    pub corrections: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Breakdown {
    pub prediction: Prediction,
    pub disagreement: f64,
    pub experts: [Prediction; 4],
    pub weights: [f64; 4],
}

#[derive(Debug, Clone)]
pub struct Ensemble {
    weights: [f64; 4],
    prior: Prior,
    linear: Linear,
    mlp: Mlp,
    neighbors: Neighbors,
    corrections: u64,
}

impl Ensemble {
    pub fn new(seed: u64) -> Self {
        Self {
            weights: INITIAL_WEIGHTS,
            prior: Prior,
            linear: Linear::default(),
            mlp: Mlp::new(seed),
            neighbors: Neighbors::default(),
            corrections: 0,
        }
    }

    pub fn restore(state: EnsembleState, seed: u64, samples: &[Sample]) -> Self {
        let weights_ok = state.weights.iter().all(|w| w.is_finite() && *w > 0.0);
        let mut ensemble = Self::new(seed);
        if weights_ok {
            ensemble.weights = normalise(state.weights);
        }
        ensemble.corrections = state.corrections;
        let data = residuals(samples);
        ensemble.linear.fit(&data);
        ensemble.neighbors.fit(&data);
        if state.mlp.is_valid() {
            ensemble.mlp = state.mlp;
        } else {
            ensemble.mlp.fit(&data);
        }
        ensemble
    }

    pub fn state(&self) -> EnsembleState {
        EnsembleState {
            weights: self.weights,
            mlp: self.mlp.clone(),
            corrections: self.corrections,
        }
    }

    pub fn linear(&self) -> &Linear {
        &self.linear
    }

    pub fn corrections(&self) -> u64 {
        self.corrections
    }

    fn experts(&self) -> [&dyn Expert; 4] {
        [&self.prior, &self.linear, &self.mlp, &self.neighbors]
    }

    pub fn predict(&self, x: &Features) -> Breakdown {
        let base = prior::mean(x);
        let experts = self.experts().map(|e| {
            let r = e.predict(x);
            Prediction {
                mean: (base + r.mean).clamp(0.0, 1.0),
                variance: r.variance,
            }
        });
        let mean: f64 = experts
            .iter()
            .zip(&self.weights)
            .map(|(e, w)| w * e.mean)
            .sum();
        let disagreement: f64 = experts
            .iter()
            .zip(&self.weights)
            .map(|(e, w)| w * (e.mean - mean).powi(2))
            .sum();
        let own: f64 = experts
            .iter()
            .zip(&self.weights)
            .map(|(e, w)| w * e.variance.min(prior::VARIANCE))
            .sum();
        Breakdown {
            prediction: Prediction {
                mean,
                variance: disagreement + own,
            },
            disagreement,
            experts,
            weights: self.weights,
        }
    }

    /// Scores every expert on the new label before any of them has seen it,
    /// then shifts weight toward the ones that were right (exponentially
    /// weighted aggregation). Must be called before `fit` for the same sample.
    pub fn observe(&mut self, x: &Features, label: f64, sample_weight: f64) {
        let predictions = self.predict(x).experts;
        for (w, p) in self.weights.iter_mut().zip(&predictions) {
            *w *= (-LEARNING_RATE * sample_weight * (p.mean - label).powi(2)).exp();
        }
        self.weights = normalise(self.weights);
        if sample_weight >= 1.0 {
            self.corrections += 1;
        }
    }

    pub fn fit(&mut self, samples: &[Sample]) {
        let data: Vec<Residual> = residuals(samples);
        self.linear.fit(&data);
        self.mlp.fit(&data);
        self.neighbors.fit(&data);
    }

    pub fn learn(&mut self, sample: &Sample, all_samples: &[Sample]) {
        if let Some(x) = sample.features() {
            self.observe(&x, sample.p, sample.weight);
        }
        self.fit(all_samples);
    }
}

fn normalise(weights: [f64; 4]) -> [f64; 4] {
    let total: f64 = weights.iter().sum();
    let mut w = if total > 0.0 && total.is_finite() {
        weights.map(|w| w / total)
    } else {
        INITIAL_WEIGHTS
    };
    for v in &mut w {
        *v = v.max(WEIGHT_FLOOR);
    }
    let total: f64 = w.iter().sum();
    w.map(|v| v / total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::DIM;

    #[test]
    fn new_ensemble_equals_prior() {
        let x = [0.1; DIM];
        let b = Ensemble::new(1).predict(&x);
        assert!((b.prediction.mean - prior::mean(&x)).abs() < 1e-9);
    }

    #[test]
    fn weight_moves_to_the_expert_that_is_right() {
        let mut e = Ensemble::new(1);
        let x = [0.0; DIM];
        let mut samples = Vec::new();
        let target = prior::mean(&x) + 0.2;
        for i in 0..8 {
            let s = Sample::new(i as f64, &x, target, 1.0);
            samples.push(s.clone());
            e.learn(&s, &samples);
        }
        let b = e.predict(&x);
        assert!(b.weights[0] < 0.2, "prior still weighted {}", b.weights[0]);
        assert!((b.prediction.mean - target).abs() < 0.03);
        assert_eq!(e.corrections(), 8);
    }

    #[test]
    fn floor_keeps_every_expert_alive() {
        let w = normalise([1.0, 1e-30, 1e-30, 1e-30]);
        assert!(w.iter().all(|v| *v >= WEIGHT_FLOOR * 0.97));
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn weak_samples_move_weights_less_and_are_not_counted() {
        let x = [0.0; DIM];
        let label = prior::mean(&x) + 0.3;
        let mut strong = Ensemble::new(1);
        let mut weak = Ensemble::new(1);
        strong.linear.fit(&[Residual {
            x,
            residual: 0.3,
            weight: 1.0,
        }]);
        weak.linear.fit(&[Residual {
            x,
            residual: 0.3,
            weight: 1.0,
        }]);
        strong.observe(&x, label, 1.0);
        weak.observe(&x, label, 0.2);
        assert!(strong.weights[1] > weak.weights[1]);
        assert_eq!(weak.corrections(), 0);
    }

    #[test]
    fn restore_rebuilds_from_samples() {
        let x = [0.0; DIM];
        let mut e = Ensemble::new(4);
        let samples = vec![Sample::new(0.0, &x, 0.9, 1.0)];
        e.learn(&samples[0], &samples);
        let restored = Ensemble::restore(e.state(), 4, &samples);
        let a = e.predict(&x).prediction.mean;
        let b = restored.predict(&x).prediction.mean;
        assert!((a - b).abs() < 1e-9);
    }

    #[test]
    fn restore_survives_garbage_weights() {
        let mut state = Ensemble::new(1).state();
        state.weights = [f64::NAN, 0.0, -1.0, 2.0];
        let e = Ensemble::restore(state, 1, &[]);
        assert_eq!(e.weights, INITIAL_WEIGHTS);
    }
}
