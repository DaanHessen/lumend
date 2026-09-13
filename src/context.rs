use crate::ambient::Ambient;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenStats {
    pub mean_luma: f64,
    pub bright_fraction: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Power {
    pub on_ac: bool,
    pub battery: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub ambient: Ambient,
    pub sun_elevation_deg: f64,
    pub local_seconds_of_day: f64,
    pub screen: Option<ScreenStats>,
    pub power: Option<Power>,
    pub night_light_kelvin: Option<u32>,
    pub video_playing: bool,
    pub fullscreen: bool,
    pub app: Option<String>,
    pub network: Option<String>,
}
