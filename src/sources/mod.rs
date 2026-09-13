pub mod hyprland;
pub mod idle;
pub mod media;
pub mod network;
pub mod nightlight;
pub mod power;
pub mod screen;
pub mod sky;

use crate::events::Event;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub fn spawn<F>(name: &str, tx: Sender<Event>, body: F) -> JoinHandle<()>
where
    F: FnOnce(Sender<Event>) + Send + 'static,
{
    thread::Builder::new()
        .name(format!("lumend-{name}"))
        .spawn(move || body(tx))
        .expect("spawning a thread only fails when the system is out of resources")
}

#[derive(Debug, Clone)]
pub struct Backoff {
    current: Duration,
    initial: Duration,
    max: Duration,
}

impl Backoff {
    pub fn new(initial: Duration, max: Duration) -> Self {
        Self {
            current: initial,
            initial,
            max,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }

    pub fn reset(&mut self) {
        self.current = self.initial;
    }
}

pub fn send_if_changed<T: PartialEq + Clone>(
    tx: &Sender<Event>,
    last: &mut Option<T>,
    value: T,
    wrap: impl FnOnce(T) -> Event,
) -> bool {
    if last.as_ref() == Some(&value) {
        return true;
    }
    *last = Some(value.clone());
    tx.send(wrap(value)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_a_cap_and_resets() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(5));
        let delays: Vec<u64> = (0..5).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 5, 5]);
        b.reset();
        assert_eq!(b.next_delay().as_secs(), 1);
    }

    #[test]
    fn duplicates_are_not_sent() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut last = None;
        send_if_changed(&tx, &mut last, true, Event::Idle);
        send_if_changed(&tx, &mut last, true, Event::Idle);
        send_if_changed(&tx, &mut last, false, Event::Idle);
        let got: Vec<Event> = rx.try_iter().collect();
        assert_eq!(got, vec![Event::Idle(true), Event::Idle(false)]);
    }
}
