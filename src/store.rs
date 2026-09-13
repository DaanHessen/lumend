use crate::model::Sample;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const MAX_SAMPLES: usize = 5000;
pub const MAX_WEAK_SHARE: f64 = 0.3;
const SAMPLES_FILE: &str = "samples.jsonl";
const STATE_FILE: &str = "model.json";

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn samples_path(&self) -> PathBuf {
        self.dir.join(SAMPLES_FILE)
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join(STATE_FILE)
    }

    pub fn load_samples(&self) -> io::Result<Vec<Sample>> {
        let file = match fs::File::open(self.samples_path()) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut samples = Vec::new();
        let mut skipped = 0usize;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Sample>(&line) {
                Ok(s) if s.features().is_some() && s.p.is_finite() => samples.push(s),
                _ => skipped += 1,
            }
        }
        if skipped > 0 {
            tracing::warn!(skipped, "ignored unreadable lines in {}", SAMPLES_FILE);
        }
        Ok(samples)
    }

    pub fn append(&self, sample: &Sample) -> io::Result<()> {
        let mut line = serde_json::to_string(sample).map_err(io::Error::other)?;
        line.push('\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.samples_path())?
            .write_all(line.as_bytes())
    }

    pub fn rewrite_samples(&self, samples: &[Sample]) -> io::Result<()> {
        let mut text = String::new();
        for s in samples {
            text.push_str(&serde_json::to_string(s).map_err(io::Error::other)?);
            text.push('\n');
        }
        write_atomic(&self.samples_path(), text.as_bytes())
    }

    pub fn load_state<T: DeserializeOwned>(&self) -> Option<T> {
        let path = self.state_path();
        let text = fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&text) {
            Ok(state) => Some(state),
            Err(e) => {
                let aside = path.with_extension("json.corrupt");
                tracing::warn!(
                    "model state unreadable ({e}), moving it to {}",
                    aside.display()
                );
                let _ = fs::rename(&path, aside);
                None
            }
        }
    }

    pub fn save_state<T: Serialize>(&self, state: &T) -> io::Result<()> {
        let text = serde_json::to_vec(state).map_err(io::Error::other)?;
        write_atomic(&self.state_path(), &text)
    }

    pub fn forget(&self) -> io::Result<()> {
        for path in [self.samples_path(), self.state_path()] {
            match fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)
}

/// Drops samples until at most `MAX_WEAK_SHARE` of them are weak and the total
/// fits in `max`. Weak samples go first, oldest first; strong samples are only
/// dropped once no weak ones are left. Returns whether anything was removed.
pub fn prune(samples: &mut Vec<Sample>, max: usize) -> bool {
    let before = samples.len();
    samples.sort_by(|a, b| a.time.total_cmp(&b.time));
    let strong = samples.iter().filter(|s| !s.is_weak()).count();
    let weak_cap = ((strong as f64) * MAX_WEAK_SHARE / (1.0 - MAX_WEAK_SHARE)).floor() as usize;
    let mut weak_seen = samples.iter().filter(|s| s.is_weak()).count();
    let mut excess = samples.len().saturating_sub(max);

    samples.retain(|s| {
        if s.is_weak() && (weak_seen > weak_cap || excess > 0) {
            weak_seen -= 1;
            excess = excess.saturating_sub(1);
            return false;
        }
        true
    });
    let overflow = samples.len().saturating_sub(max);
    samples.drain(..overflow);
    samples.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::DIM;

    fn sample(time: f64, weight: f64) -> Sample {
        Sample::new(time, &[0.0; DIM], 0.5, weight)
    }

    #[test]
    fn append_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state")).unwrap();
        assert!(store.load_samples().unwrap().is_empty());
        store.append(&sample(1.0, 1.0)).unwrap();
        store.append(&sample(2.0, 0.2)).unwrap();
        let loaded = store.load_samples().unwrap();
        assert_eq!(loaded, vec![sample(1.0, 1.0), sample(2.0, 0.2)]);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.append(&sample(1.0, 1.0)).unwrap();
        let mut f = OpenOptions::new()
            .append(true)
            .open(store.samples_path())
            .unwrap();
        writeln!(f, "{{not json").unwrap();
        writeln!(f, "{{\"time\":1,\"x\":[1,2],\"p\":0.5,\"weight\":1}}").unwrap();
        store.append(&sample(3.0, 1.0)).unwrap();
        assert_eq!(store.load_samples().unwrap().len(), 2);
    }

    #[test]
    fn corrupt_state_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(store.state_path(), "garbage").unwrap();
        assert!(store.load_state::<Vec<u32>>().is_none());
        assert!(dir.path().join("model.json.corrupt").exists());
        store.save_state(&vec![1u32, 2]).unwrap();
        assert_eq!(store.load_state::<Vec<u32>>(), Some(vec![1, 2]));
    }

    #[test]
    fn forget_removes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.append(&sample(1.0, 1.0)).unwrap();
        store.save_state(&1u8).unwrap();
        store.forget().unwrap();
        store.forget().unwrap();
        assert!(store.load_samples().unwrap().is_empty());
        assert!(store.load_state::<u8>().is_none());
    }

    #[test]
    fn prune_caps_weak_share() {
        let mut samples: Vec<Sample> = (0..10).map(|t| sample(t as f64, 1.0)).collect();
        samples.extend((10..20).map(|t| sample(t as f64, 0.2)));
        assert!(prune(&mut samples, MAX_SAMPLES));
        let weak = samples.iter().filter(|s| s.is_weak()).count();
        assert_eq!(weak, 4);
        assert!(
            samples
                .iter()
                .filter(|s| s.is_weak())
                .all(|s| s.time >= 16.0)
        );
    }

    #[test]
    fn prune_drops_weak_before_strong_when_full() {
        let mut samples: Vec<Sample> = (0..8).map(|t| sample(t as f64, 1.0)).collect();
        samples.push(sample(8.0, 0.2));
        samples.push(sample(9.0, 0.2));
        prune(&mut samples, 8);
        assert_eq!(samples.len(), 8);
        assert!(samples.iter().all(|s| !s.is_weak()));

        prune(&mut samples, 5);
        assert_eq!(
            samples.iter().map(|s| s.time as u32).collect::<Vec<_>>(),
            vec![3, 4, 5, 6, 7]
        );
    }

    #[test]
    fn prune_leaves_small_sets_alone() {
        let mut samples = vec![sample(0.0, 1.0), sample(1.0, 1.0)];
        assert!(!prune(&mut samples, MAX_SAMPLES));
    }
}
