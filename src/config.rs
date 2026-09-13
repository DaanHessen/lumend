use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub backlight: BacklightConfig,
    pub location: LocationConfig,
    pub sky: SkyConfig,
    pub screen: ScreenConfig,
    pub learning: LearningConfig,
    pub transitions: TransitionConfig,
    pub idle: IdleConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BacklightConfig {
    pub device: Option<String>,
    pub poll_hz: f64,
    pub min_level: u32,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LocationConfig {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SkyConfig {
    pub enabled: bool,
    pub interval_minutes: u64,
    pub daylight_factor: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ScreenConfig {
    pub enabled: bool,
    pub interval_seconds: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    pub settle_seconds: f64,
    pub weak_confirmations: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TransitionConfig {
    pub brighten_rate: f64,
    pub dim_rate: f64,
    pub brighten_delay_seconds: f64,
    pub dim_delay_seconds: f64,
    pub break_step: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct IdleConfig {
    pub enabled: bool,
    pub dim_after_seconds: u32,
    pub dim_by: f64,
}

impl Default for BacklightConfig {
    fn default() -> Self {
        Self {
            device: None,
            poll_hz: 5.0,
            min_level: 1,
        }
    }
}

impl Default for SkyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_minutes: 10,
            daylight_factor: 0.02,
        }
    }
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 3.0,
        }
    }
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            settle_seconds: 30.0,
            weak_confirmations: true,
        }
    }
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            brighten_rate: 0.15,
            dim_rate: 0.02,
            brighten_delay_seconds: 4.0,
            dim_delay_seconds: 8.0,
            break_step: 0.15,
        }
    }
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dim_after_seconds: 90,
            dim_by: 0.2,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            Self::Parse(path, e) => write!(f, "invalid config {}: {e}", path.display()),
            Self::Invalid(msg) => write!(f, "invalid config: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn default_path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| match e {
                ConfigError::Parse(_, inner) => ConfigError::Parse(path.to_path_buf(), inner),
                other => other,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Read(path.to_path_buf(), e)),
        }
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(text).map_err(|e| ConfigError::Parse(PathBuf::new(), e))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |msg: &str| Err(ConfigError::Invalid(msg.to_owned()));
        if self.location.latitude.is_some() != self.location.longitude.is_some() {
            return invalid("location needs both latitude and longitude");
        }
        if let Some(lat) = self.location.latitude
            && !(-90.0..=90.0).contains(&lat)
        {
            return invalid("latitude must be between -90 and 90");
        }
        if let Some(lon) = self.location.longitude
            && !(-180.0..=180.0).contains(&lon)
        {
            return invalid("longitude must be between -180 and 180");
        }
        if !(1.0..=50.0).contains(&self.backlight.poll_hz) {
            return invalid("backlight.poll_hz must be between 1 and 50");
        }
        if !(5.0..=300.0).contains(&self.learning.settle_seconds) {
            return invalid("learning.settle_seconds must be between 5 and 300");
        }
        if self.sky.interval_minutes < 5 {
            return invalid("sky.interval_minutes must be at least 5");
        }
        if !(0.001..=0.2).contains(&self.sky.daylight_factor) {
            return invalid("sky.daylight_factor must be between 0.001 and 0.2");
        }
        if self.screen.interval_seconds < 0.5 {
            return invalid("screen.interval_seconds must be at least 0.5");
        }
        let t = &self.transitions;
        if t.brighten_rate <= 0.0 || t.dim_rate <= 0.0 {
            return invalid("transition rates must be positive");
        }
        if !(0.0..=1.0).contains(&t.break_step) || !(0.0..=1.0).contains(&self.idle.dim_by) {
            return invalid("break_step and idle.dim_by must be between 0 and 1");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config =
            Config::parse("[sky]\nenabled = false\n[learning]\nsettle_seconds = 15\n").unwrap();
        assert!(!config.sky.enabled);
        assert_eq!(config.sky.interval_minutes, 10);
        assert_eq!(config.learning.settle_seconds, 15.0);
        assert_eq!(config.transitions, TransitionConfig::default());
    }

    #[test]
    fn rejects_half_a_location() {
        assert!(Config::parse("[location]\nlatitude = 52.0\n").is_err());
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("[sky]\nenabeld = false\n").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(config, Config::default());
    }
}
