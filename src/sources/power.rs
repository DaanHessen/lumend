use crate::context::Power;
use crate::events::Event;
use std::fs;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

const ROOT: &str = "/sys/class/power_supply";
const INTERVAL: Duration = Duration::from_secs(30);

fn read(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name))
        .ok()
        .map(|s| s.trim().to_owned())
}

fn number(dir: &Path, name: &str) -> Option<f64> {
    read(dir, name)?.parse().ok()
}

fn battery_fraction(dir: &Path) -> Option<f64> {
    if let (Some(now), Some(full)) = (number(dir, "energy_now"), number(dir, "energy_full")) {
        return (full > 0.0).then(|| (now / full).clamp(0.0, 1.0));
    }
    if let (Some(now), Some(full)) = (number(dir, "charge_now"), number(dir, "charge_full")) {
        return (full > 0.0).then(|| (now / full).clamp(0.0, 1.0));
    }
    number(dir, "capacity").map(|c| (c / 100.0).clamp(0.0, 1.0))
}

pub fn snapshot(root: &Path) -> Option<Power> {
    let mut on_ac = None;
    let mut batteries = Vec::new();
    for entry in fs::read_dir(root).ok()?.flatten() {
        let dir = entry.path();
        match read(&dir, "type").as_deref() {
            Some("Mains") | Some("USB") => {
                if read(&dir, "online").as_deref() == Some("1") {
                    on_ac = Some(true);
                } else {
                    on_ac.get_or_insert(false);
                }
            }
            Some("Battery") if read(&dir, "scope").as_deref() != Some("Device") => {
                if let Some(f) = battery_fraction(&dir) {
                    batteries.push(f);
                }
            }
            _ => {}
        }
    }
    if on_ac.is_none() && batteries.is_empty() {
        return None;
    }
    let battery =
        (!batteries.is_empty()).then(|| batteries.iter().sum::<f64>() / batteries.len() as f64);
    Some(Power {
        on_ac: on_ac.unwrap_or(battery.is_none()),
        battery: battery.map(|b| (b * 100.0).round() / 100.0),
    })
}

pub fn run(tx: Sender<Event>) {
    let mut last = None;
    loop {
        let power = snapshot(Path::new(ROOT));
        if !super::send_if_changed(&tx, &mut last, power, Event::Power) {
            return;
        }
        std::thread::sleep(INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        for (file, value) in files {
            fs::write(dir.join(file), format!("{value}\n")).unwrap();
        }
    }

    #[test]
    fn laptop_on_ac() {
        let root = tempfile::tempdir().unwrap();
        supply(root.path(), "ACAD", &[("type", "Mains"), ("online", "1")]);
        supply(
            root.path(),
            "BAT0",
            &[
                ("type", "Battery"),
                ("energy_now", "45000000"),
                ("energy_full", "90000000"),
            ],
        );
        assert_eq!(
            snapshot(root.path()),
            Some(Power {
                on_ac: true,
                battery: Some(0.5)
            })
        );
    }

    #[test]
    fn on_battery_with_capacity_only_and_a_mouse_battery() {
        let root = tempfile::tempdir().unwrap();
        supply(root.path(), "AC", &[("type", "Mains"), ("online", "0")]);
        supply(
            root.path(),
            "BAT1",
            &[("type", "Battery"), ("capacity", "83")],
        );
        supply(
            root.path(),
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device"), ("capacity", "5")],
        );
        assert_eq!(
            snapshot(root.path()),
            Some(Power {
                on_ac: false,
                battery: Some(0.83)
            })
        );
    }

    #[test]
    fn desktop_without_supplies() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(snapshot(root.path()), None);
    }
}
