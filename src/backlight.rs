use crate::events::Event;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

pub const SYSFS_ROOT: &str = "/sys/class/backlight";
const TYPE_PREFERENCE: [&str; 3] = ["firmware", "platform", "raw"];

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub path: PathBuf,
    pub max: u32,
}

fn read_u32(path: &Path) -> io::Result<u32> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn discover(root: &Path, wanted: Option<&str>) -> io::Result<Device> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if wanted.is_some_and(|w| w != name) {
            continue;
        }
        let Ok(max) = read_u32(&path.join("max_brightness")) else {
            continue;
        };
        let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
        let rank = TYPE_PREFERENCE
            .iter()
            .position(|t| *t == kind.trim())
            .unwrap_or(TYPE_PREFERENCE.len());
        if max > 0 {
            candidates.push((rank, std::cmp::Reverse(max), Device { name, path, max }));
        }
    }
    candidates.sort_by_key(|c| (c.0, c.1));
    candidates
        .into_iter()
        .next()
        .map(|(_, _, d)| d)
        .ok_or_else(|| {
            let what = wanted.map_or("any backlight".to_owned(), |w| format!("backlight {w}"));
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no {what} under {}", root.display()),
            )
        })
}

impl Device {
    pub fn level(&self) -> io::Result<u32> {
        read_u32(&self.path.join("actual_brightness"))
            .or_else(|_| read_u32(&self.path.join("brightness")))
            .map(|l| l.min(self.max))
    }
}

pub struct Writer {
    device: Device,
    session: Option<Proxy<'static>>,
}

impl Writer {
    pub fn new(device: Device) -> Self {
        let session = match session_proxy() {
            Ok(proxy) => Some(proxy),
            Err(e) => {
                tracing::warn!("logind unavailable ({e}), will write sysfs directly");
                None
            }
        };
        Self { device, session }
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn set(&self, level: u32) -> io::Result<()> {
        let level = level.min(self.device.max);
        if let Some(proxy) = &self.session {
            let args = ("backlight", self.device.name.as_str(), level);
            match proxy.call::<_, _, ()>("SetBrightness", &args) {
                Ok(()) => return Ok(()),
                Err(e) => tracing::debug!("logind SetBrightness failed: {e}"),
            }
        }
        fs::write(self.device.path.join("brightness"), level.to_string())
    }
}

fn session_proxy() -> zbus::Result<Proxy<'static>> {
    const LOGIN1: &str = "org.freedesktop.login1";
    let conn = Connection::system()?;
    let user = Proxy::new(
        &conn,
        LOGIN1,
        "/org/freedesktop/login1/user/self",
        "org.freedesktop.login1.User",
    )?;
    let session = match user.get_property::<(String, OwnedObjectPath)>("Display") {
        Ok((_, path)) if path.as_str() != "/" => path,
        _ => OwnedObjectPath::try_from("/org/freedesktop/login1/session/auto")?,
    };
    Proxy::new(&conn, LOGIN1, session, "org.freedesktop.login1.Session")
}

pub fn poll(device: Device, hz: f64, tx: Sender<Event>) {
    let period = Duration::from_secs_f64(1.0 / hz);
    let mut last = None;
    let mut failures = 0u32;
    loop {
        match device.level() {
            Ok(level) => {
                failures = 0;
                if !crate::sources::send_if_changed(&tx, &mut last, level, Event::Backlight) {
                    return;
                }
            }
            Err(e) => {
                failures += 1;
                if failures == 1 || failures.is_power_of_two() {
                    tracing::warn!("reading {} failed: {e}", device.name);
                }
            }
        }
        std::thread::sleep(period);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(root: &Path, name: &str, kind: &str, max: u32, level: u32) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
        fs::write(dir.join("max_brightness"), format!("{max}\n")).unwrap();
        fs::write(dir.join("actual_brightness"), format!("{level}\n")).unwrap();
    }

    #[test]
    fn prefers_firmware_then_platform_then_raw() {
        let root = tempfile::tempdir().unwrap();
        fake(root.path(), "intel_backlight", "raw", 96000, 1000);
        fake(root.path(), "acpi_video0", "platform", 10, 5);
        fake(root.path(), "nvidia_wmi_ec_backlight", "firmware", 100, 40);
        let d = discover(root.path(), None).unwrap();
        assert_eq!(d.name, "nvidia_wmi_ec_backlight");
        assert_eq!(d.max, 100);
        assert_eq!(d.level().unwrap(), 40);
    }

    #[test]
    fn honours_configured_device() {
        let root = tempfile::tempdir().unwrap();
        fake(root.path(), "intel_backlight", "raw", 96000, 1000);
        fake(root.path(), "nvidia_wmi_ec_backlight", "firmware", 100, 40);
        let d = discover(root.path(), Some("intel_backlight")).unwrap();
        assert_eq!(d.max, 96000);
        assert!(discover(root.path(), Some("missing")).is_err());
    }

    #[test]
    fn falls_back_to_brightness_and_clamps() {
        let root = tempfile::tempdir().unwrap();
        fake(root.path(), "x", "raw", 100, 0);
        fs::remove_file(root.path().join("x/actual_brightness")).unwrap();
        fs::write(root.path().join("x/brightness"), "250\n").unwrap();
        assert_eq!(discover(root.path(), None).unwrap().level().unwrap(), 100);
    }

    #[test]
    fn empty_root_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(discover(root.path(), None).is_err());
    }
}
