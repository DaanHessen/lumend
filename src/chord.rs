use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const WINDOW_SECONDS: f64 = 0.12;
const QUIET_SECONDS: f64 = 0.4;
const COOLDOWN_SECONDS: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Up,
    Down,
}

#[derive(Debug, Default)]
pub struct ChordDetector {
    presses: VecDeque<(Key, f64)>,
    last_chord: Option<f64>,
}

impl ChordDetector {
    /// Returns true when this press completes a chord: the other brightness key
    /// was pressed at most 120 ms earlier, nothing else was pressed in the
    /// 400 ms before that, and no chord fired in the last second. Pressing the
    /// keys one after the other, or tapping them in a quick run, never matches.
    pub fn press(&mut self, key: Key, now: f64) -> bool {
        let fired = match self.presses.back() {
            Some(&(previous, at)) if previous != key && now - at <= WINDOW_SECONDS => {
                let quiet_before = self
                    .presses
                    .iter()
                    .rev()
                    .nth(1)
                    .is_none_or(|&(_, t)| at - t >= QUIET_SECONDS);
                let cooled = self.last_chord.is_none_or(|t| now - t >= COOLDOWN_SECONDS);
                quiet_before && cooled
            }
            _ => false,
        };
        if fired {
            self.last_chord = Some(now);
            self.presses.clear();
        } else {
            if self.presses.len() == 3 {
                self.presses.pop_front();
            }
            self.presses.push_back((key, now));
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::{Down, Up};

    fn run(presses: &[(Key, f64)]) -> Vec<bool> {
        let mut d = ChordDetector::default();
        presses.iter().map(|&(k, t)| d.press(k, t)).collect()
    }

    #[test]
    fn both_keys_together_fire_in_either_order() {
        assert_eq!(run(&[(Up, 10.0), (Down, 10.03)]), vec![false, true]);
        assert_eq!(run(&[(Down, 10.0), (Up, 10.1)]), vec![false, true]);
    }

    #[test]
    fn one_after_the_other_does_not_fire() {
        assert_eq!(run(&[(Down, 10.0), (Up, 10.2)]), vec![false, false]);
        assert_eq!(run(&[(Up, 10.0), (Down, 10.5), (Up, 11.0)]), vec![false; 3]);
    }

    #[test]
    fn same_key_twice_does_not_fire() {
        assert_eq!(run(&[(Up, 10.0), (Up, 10.05)]), vec![false, false]);
    }

    #[test]
    fn a_quick_run_of_presses_does_not_fire() {
        let presses = [
            (Up, 10.0),
            (Up, 10.25),
            (Down, 10.33),
            (Up, 10.4),
            (Down, 10.5),
        ];
        assert_eq!(run(&presses), vec![false; 5]);
    }

    #[test]
    fn a_second_chord_needs_a_pause() {
        let presses = [
            (Up, 10.0),
            (Down, 10.05),
            (Up, 10.6),
            (Down, 10.65),
            (Up, 12.0),
            (Down, 12.04),
        ];
        assert_eq!(run(&presses), vec![false, true, false, false, false, true]);
    }
}
