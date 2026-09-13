use crate::events::Event;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(60);

pub fn parse_temperature(output: &str) -> Option<u32> {
    let kelvin: u32 = output.trim().parse().ok()?;
    (1000..=20000).contains(&kelvin).then_some(kelvin)
}

fn query() -> Option<u32> {
    let output = Command::new("hyprctl")
        .args(["hyprsunset", "temperature"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_temperature(&String::from_utf8_lossy(&output.stdout))
}

pub fn run(tx: Sender<Event>) {
    let mut last = None;
    loop {
        if !super::send_if_changed(&tx, &mut last, query(), Event::NightLight) {
            return;
        }
        std::thread::sleep(INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hyprsunset_output() {
        assert_eq!(parse_temperature("5000\n"), Some(5000));
        assert_eq!(parse_temperature("6000"), Some(6000));
        assert_eq!(parse_temperature("couldn't connect to hyprsunset"), None);
        assert_eq!(parse_temperature("0"), None);
    }
}
