use crate::config::Config;
use crate::model::Prediction;
use crate::perceptual;
use std::collections::VecDeque;

const BASE_DEADBAND: f64 = 0.03;
const UNCERTAINTY_DEADBAND: f64 = 0.5;
const ARRIVED: f64 = 0.004;
const HOLD_SECONDS: f64 = 180.0;
const WEAK_INTERVAL_SECONDS: f64 = 1200.0;
const BREAK_WINDOW_SECONDS: f64 = 1.5;
const TIME_JUMP_SECONDS: f64 = 10.0;
const RESYNC_SECONDS: f64 = 5.0;
const MAX_STEP_SECONDS: f64 = 1.0;
const ECHO_HISTORY: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub brighten_rate: f64,
    pub dim_rate: f64,
    pub brighten_delay: f64,
    pub dim_delay: f64,
    pub break_step: f64,
    pub settle: f64,
    pub idle_dim_by: f64,
    pub weak_confirmations: bool,
    pub min_level: u32,
}

impl Settings {
    pub fn from_config(config: &Config) -> Self {
        let t = &config.transitions;
        Self {
            brighten_rate: t.brighten_rate,
            dim_rate: t.dim_rate,
            brighten_delay: t.brighten_delay_seconds,
            dim_delay: t.dim_delay_seconds,
            break_step: t.break_step,
            settle: config.learning.settle_seconds,
            idle_dim_by: if config.idle.enabled {
                config.idle.dim_by
            } else {
                0.0
            },
            weak_confirmations: config.learning.weak_confirmations,
            min_level: config.backlight.min_level,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Auto,
    Adjusting { last_change: f64 },
    Held { since: f64 },
    Paused { until: Option<f64> },
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Adjusting { .. } => "adjusting",
            Self::Held { .. } => "holding your setting",
            Self::Paused { .. } => "paused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Set(u32),
    Correction(f64),
    Confirmation(f64),
}

#[derive(Debug, Clone)]
pub struct Controller {
    settings: Settings,
    max_level: u32,
    mode: Mode,
    level: u32,
    position: f64,
    written: VecDeque<u32>,
    pending: Option<(f64, f64)>,
    ramping: bool,
    idle: bool,
    break_until: f64,
    instant_restore: bool,
    resync_until: f64,
    last_tick: Option<f64>,
    last_label: f64,
}

impl Controller {
    pub fn new(settings: Settings, max_level: u32, level: u32, now: f64) -> Self {
        let mut written = VecDeque::with_capacity(ECHO_HISTORY);
        written.push_back(level);
        Self {
            settings,
            max_level,
            mode: Mode::Auto,
            level,
            position: perceptual::to_p(level, max_level),
            written,
            pending: None,
            ramping: false,
            idle: false,
            break_until: 0.0,
            instant_restore: false,
            resync_until: 0.0,
            last_tick: None,
            last_label: now,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    fn min_p(&self) -> f64 {
        perceptual::to_p(self.settings.min_level, self.max_level)
    }

    pub fn observe_level(&mut self, level: u32, now: f64) {
        if self.written.contains(&level) {
            return;
        }
        let external =
            self.idle || now < self.resync_until || matches!(self.mode, Mode::Paused { .. });
        self.level = level;
        self.position = perceptual::to_p(level, self.max_level);
        self.written.clear();
        self.written.push_back(level);
        self.ramping = false;
        self.pending = None;
        if !external {
            self.mode = Mode::Adjusting { last_change: now };
        }
    }

    pub fn set_idle(&mut self, idle: bool, now: f64) {
        if idle == self.idle {
            return;
        }
        self.idle = idle;
        if !idle {
            self.instant_restore = true;
            self.break_until = now + BREAK_WINDOW_SECONDS;
        }
    }

    pub fn break_moment(&mut self, now: f64) {
        self.break_until = now + BREAK_WINDOW_SECONDS;
    }

    pub fn pause(&mut self, until: Option<f64>) {
        self.mode = Mode::Paused { until };
        self.ramping = false;
        self.pending = None;
    }

    pub fn resume(&mut self) {
        if matches!(self.mode, Mode::Paused { .. }) {
            self.mode = Mode::Auto;
        }
    }

    pub fn ignore_level_changes_until(&mut self, until: f64) {
        self.resync_until = self.resync_until.max(until);
    }

    pub fn tick(&mut self, now: f64, target: Prediction) -> Vec<Action> {
        let dt = self.last_tick.map_or(0.0, |t| now - t);
        self.last_tick = Some(now);
        if dt > TIME_JUMP_SECONDS {
            self.resync_until = now + RESYNC_SECONDS;
            self.pending = None;
        }

        match self.mode {
            Mode::Paused { until: Some(until) } if now >= until => self.mode = Mode::Auto,
            Mode::Paused { .. } => return Vec::new(),
            Mode::Adjusting { last_change } => {
                if now - last_change < self.settings.settle {
                    return Vec::new();
                }
                self.mode = Mode::Held { since: now };
                self.last_label = now;
                return vec![Action::Correction(self.position)];
            }
            Mode::Held { since } if now - since < HOLD_SECONDS => return Vec::new(),
            Mode::Held { .. } => self.mode = Mode::Auto,
            Mode::Auto => {}
        }

        self.steer(now, dt.clamp(0.0, MAX_STEP_SECONDS), target)
    }

    /// Moves `position` toward the target. A move only starts once the target
    /// has stayed on the same side of the deadband for the brighten or dim
    /// delay, except at a break moment or when returning from idle. Once
    /// started, the ramp follows the target until it arrives.
    fn steer(&mut self, now: f64, dt: f64, target: Prediction) -> Vec<Action> {
        let s = &self.settings;
        let offset = if self.idle { s.idle_dim_by } else { 0.0 };
        let goal = (target.mean - offset).clamp(self.min_p(), 1.0);
        let diff = goal - self.position;
        let in_break = now < self.break_until;
        let fast_start = in_break || self.instant_restore;

        if !self.ramping {
            let deadband = BASE_DEADBAND + UNCERTAINTY_DEADBAND * target.variance.max(0.0).sqrt();
            if diff.abs() < deadband {
                self.pending = None;
                self.instant_restore = false;
                return self.maybe_confirm(now);
            }
            let direction = diff.signum();
            let delay = if direction > 0.0 {
                s.brighten_delay
            } else {
                s.dim_delay
            };
            let since = match self.pending {
                Some((d, since)) if d == direction => since,
                _ => {
                    self.pending = Some((direction, now));
                    now
                }
            };
            if !fast_start && now - since < delay {
                return Vec::new();
            }
            self.ramping = true;
            self.pending = None;
        }

        let rate = if diff > 0.0 {
            s.brighten_rate
        } else {
            s.dim_rate
        };
        let limit = if self.instant_restore {
            1.0
        } else if in_break {
            s.break_step.max(rate * dt)
        } else {
            rate * dt
        };
        self.instant_restore = false;
        self.break_until = 0.0;
        self.position += diff.clamp(-limit, limit);
        if (goal - self.position).abs() < ARRIVED {
            self.ramping = false;
        }

        let level =
            perceptual::to_level(self.position, self.max_level).max(self.settings.min_level);
        if level == self.level {
            return Vec::new();
        }
        self.level = level;
        if self.written.len() == ECHO_HISTORY {
            self.written.pop_front();
        }
        self.written.push_back(level);
        vec![Action::Set(level)]
    }

    fn maybe_confirm(&mut self, now: f64) -> Vec<Action> {
        if !self.settings.weak_confirmations
            || self.idle
            || now - self.last_label < WEAK_INTERVAL_SECONDS
        {
            return Vec::new();
        }
        self.last_label = now;
        vec![Action::Confirmation(self.position)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u32 = 100;

    fn settings() -> Settings {
        Settings::from_config(&Config::default())
    }

    fn target(p: f64) -> Prediction {
        Prediction {
            mean: p,
            variance: 0.0,
        }
    }

    fn run(c: &mut Controller, from: f64, to: f64, step: f64, p: f64) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut t = from;
        while t <= to + 1e-9 {
            actions.extend(c.tick(t, target(p)));
            t += step;
        }
        actions
    }

    fn sets(actions: &[Action]) -> Vec<u32> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Set(l) => Some(*l),
                _ => None,
            })
            .collect()
    }

    fn at(level: u32) -> f64 {
        perceptual::to_p(level, MAX)
    }

    #[test]
    fn stays_put_inside_the_deadband() {
        let mut c = Controller::new(settings(), MAX, 40, 0.0);
        assert!(run(&mut c, 0.0, 30.0, 0.25, at(40) + 0.02).is_empty());
    }

    #[test]
    fn uncertainty_widens_the_deadband() {
        let mut c = Controller::new(settings(), MAX, 40, 0.0);
        let p = at(40) + 0.06;
        let unsure = Prediction {
            mean: p,
            variance: 0.02,
        };
        for i in 0..40 {
            assert!(c.tick(i as f64 * 0.25, unsure).is_empty());
        }
        assert!(!run(&mut c, 10.0, 20.0, 0.25, p).is_empty());
    }

    #[test]
    fn brightening_waits_four_seconds_then_ramps() {
        let mut c = Controller::new(settings(), MAX, 20, 0.0);
        assert!(run(&mut c, 0.0, 3.75, 0.25, at(60)).is_empty());
        let actions = run(&mut c, 4.0, 10.0, 0.25, at(60));
        let levels = sets(&actions);
        assert!(!levels.is_empty());
        assert!(levels.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(*levels.last().unwrap(), 60);
    }

    #[test]
    fn dimming_waits_eight_seconds_and_is_slow() {
        let mut c = Controller::new(settings(), MAX, 60, 0.0);
        assert!(run(&mut c, 0.0, 7.75, 0.25, at(20)).is_empty());
        let after_ten_seconds = sets(&run(&mut c, 8.0, 18.0, 0.25, at(20)));
        let dropped = at(60) - at(*after_ten_seconds.last().unwrap());
        let allowed = settings().dim_rate * 10.0;
        assert!(
            dropped <= allowed + 0.01,
            "dropped {dropped} p in ten seconds"
        );
        let later = sets(&run(&mut c, 18.25, 180.0, 0.25, at(20)));
        assert_eq!(*later.last().unwrap(), 20);
    }

    #[test]
    fn break_moment_allows_an_immediate_step() {
        let mut c = Controller::new(settings(), MAX, 20, 0.0);
        c.tick(0.0, target(at(20)));
        c.break_moment(0.1);
        let levels = sets(&c.tick(0.25, target(at(60))));
        assert_eq!(levels.len(), 1);
        let jumped = at(levels[0]) - at(20);
        assert!((jumped - 0.15).abs() < 0.02, "jumped {jumped}");
    }

    #[test]
    fn own_writes_are_not_corrections() {
        let mut c = Controller::new(settings(), MAX, 20, 0.0);
        let mut writes = 0;
        for i in 0..40 {
            let now = i as f64 * 0.25;
            for action in c.tick(now, target(at(60))) {
                if let Action::Set(level) = action {
                    writes += 1;
                    c.observe_level(level, now + 0.1);
                }
            }
        }
        assert!(writes > 3);
        assert_eq!(c.mode(), Mode::Auto);
        assert_eq!(c.level(), 60);
    }

    #[test]
    fn user_change_becomes_a_correction_after_settling() {
        let mut s = settings();
        s.settle = 30.0;
        let mut c = Controller::new(s, MAX, 40, 0.0);
        c.tick(0.0, target(at(40)));
        c.observe_level(50, 1.0);
        c.observe_level(60, 2.0);
        assert!(matches!(c.mode(), Mode::Adjusting { .. }));
        assert!(
            run(&mut c, 2.25, 31.75, 0.25, at(40)).is_empty(),
            "correction before settling"
        );
        let actions = c.tick(32.0, target(at(40)));
        assert_eq!(actions, vec![Action::Correction(at(60))]);
        assert!(matches!(c.mode(), Mode::Held { .. }));
        assert!(run(&mut c, 32.25, 32.0 + HOLD_SECONDS - 0.5, 0.25, at(40)).is_empty());
        let after = run(
            &mut c,
            32.0 + HOLD_SECONDS,
            32.0 + HOLD_SECONDS + 60.0,
            0.25,
            at(40),
        );
        assert!(!sets(&after).is_empty(), "never left the held state");
    }

    #[test]
    fn idle_dims_slowly_and_activity_restores_at_once() {
        let mut c = Controller::new(settings(), MAX, 60, 0.0);
        let p = at(60);
        run(&mut c, 0.0, 1.0, 0.25, p);
        c.set_idle(true, 1.0);
        let dimmed = sets(&run(&mut c, 1.25, 60.0, 0.25, p));
        assert!(at(*dimmed.last().unwrap()) < p - 0.15);
        c.set_idle(false, 60.1);
        let restored = sets(&c.tick(60.25, target(p)));
        assert_eq!(restored, vec![60]);
    }

    #[test]
    fn changes_while_idle_or_after_resume_are_not_corrections() {
        let mut c = Controller::new(settings(), MAX, 60, 0.0);
        c.set_idle(true, 0.0);
        c.observe_level(30, 1.0);
        assert_eq!(c.mode(), Mode::Auto);
        c.set_idle(false, 2.0);
        c.tick(2.0, target(at(30)));
        c.tick(3600.0, target(at(30)));
        c.observe_level(80, 3601.0);
        assert_eq!(
            c.mode(),
            Mode::Auto,
            "a firmware reset after suspend was learned"
        );
        c.observe_level(70, 3610.0);
        assert!(matches!(c.mode(), Mode::Adjusting { .. }));
    }

    #[test]
    fn pause_blocks_writes_and_learning_until_it_expires() {
        let mut c = Controller::new(settings(), MAX, 20, 0.0);
        c.pause(Some(100.0));
        assert!(run(&mut c, 0.0, 99.0, 0.25, at(80)).is_empty());
        c.observe_level(35, 50.0);
        assert!(matches!(c.mode(), Mode::Paused { .. }));
        assert!(!run(&mut c, 100.0, 120.0, 0.25, at(80)).is_empty());
        assert_eq!(c.mode(), Mode::Auto);
    }

    #[test]
    fn steady_use_produces_weak_confirmations() {
        let mut c = Controller::new(settings(), MAX, 40, 0.0);
        let actions = run(&mut c, 0.0, 2500.0, 1.0, at(40));
        let confirmations = actions
            .iter()
            .filter(|a| matches!(a, Action::Confirmation(_)))
            .count();
        assert_eq!(confirmations, 2);
    }

    #[test]
    fn never_goes_below_the_minimum_level() {
        let mut c = Controller::new(settings(), MAX, 10, 0.0);
        let levels = sets(&run(&mut c, 0.0, 600.0, 0.25, 0.0));
        assert_eq!(*levels.last().unwrap(), 1);
    }
}
