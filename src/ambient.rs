use crate::solar::{self, SunPosition};

const LUMINOUS_EFFICACY: f64 = 110.0;
const ARTIFICIAL_FLOOR_LUX: f64 = 80.0;
const UNKNOWN_CLEAR_SKY_INDEX: f64 = 0.6;
const SATELLITE_MAX_AGE_S: f64 = 40.0 * 60.0;
const FORECAST_MAX_AGE_S: f64 = 90.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Irradiance {
    pub ghi: f64,
    pub observed_at: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sky {
    pub satellite: Option<Irradiance>,
    pub forecast_now: Option<Irradiance>,
    pub forecast_at_satellite_time: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GhiSource {
    Satellite,
    Forecast,
    ClearSkyGuess,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ambient {
    pub lux: f64,
    pub ghi: f64,
    pub clear_sky_ghi: f64,
    pub clear_sky_index: Option<f64>,
    pub source: GhiSource,
}

pub fn estimate(sun: SunPosition, sky: &Sky, now: f64, daylight_factor: f64) -> Ambient {
    let clear = solar::clear_sky_ghi(sun);
    let fresh = |r: &Irradiance, max_age: f64| now - r.observed_at <= max_age;

    let (ghi, source) = match (sky.satellite, sky.forecast_now) {
        (Some(sat), forecast) if fresh(&sat, SATELLITE_MAX_AGE_S) => {
            let drift = match (forecast, sky.forecast_at_satellite_time) {
                (Some(now_fc), Some(then_fc)) if fresh(&now_fc, FORECAST_MAX_AGE_S) => {
                    now_fc.ghi - then_fc
                }
                _ => 0.0,
            };
            ((sat.ghi + drift).max(0.0), GhiSource::Satellite)
        }
        (_, Some(fc)) if fresh(&fc, FORECAST_MAX_AGE_S) => (fc.ghi.max(0.0), GhiSource::Forecast),
        _ => (clear * UNKNOWN_CLEAR_SKY_INDEX, GhiSource::ClearSkyGuess),
    };

    let clear_sky_index = match source {
        GhiSource::ClearSkyGuess => None,
        _ if clear > 25.0 => Some((ghi / clear).clamp(0.0, 1.2)),
        _ => None,
    };

    let daylight = ghi * LUMINOUS_EFFICACY * daylight_factor;
    Ambient {
        lux: daylight.max(ARTIFICIAL_FLOOR_LUX),
        ghi,
        clear_sky_ghi: clear,
        clear_sky_index,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.0;
    const NOON: SunPosition = SunPosition {
        elevation_deg: 45.0,
        azimuth_deg: 180.0,
    };
    const NIGHT: SunPosition = SunPosition {
        elevation_deg: -20.0,
        azimuth_deg: 0.0,
    };

    fn reading(ghi: f64, age: f64) -> Option<Irradiance> {
        Some(Irradiance {
            ghi,
            observed_at: NOW - age,
        })
    }

    #[test]
    fn night_is_the_artificial_floor() {
        let a = estimate(NIGHT, &Sky::default(), NOW, 0.02);
        assert_eq!(a.lux, ARTIFICIAL_FLOOR_LUX);
        assert_eq!(a.clear_sky_index, None);
    }

    #[test]
    fn clear_noon_is_bright() {
        let sky = Sky {
            satellite: reading(700.0, 600.0),
            ..Sky::default()
        };
        let a = estimate(NOON, &sky, NOW, 0.02);
        assert_eq!(a.source, GhiSource::Satellite);
        assert!((a.lux - 1540.0).abs() < 1.0);
        assert!(a.clear_sky_index.unwrap() > 0.9);
    }

    #[test]
    fn stale_satellite_falls_back_to_forecast_then_guess() {
        let sky = Sky {
            satellite: reading(700.0, 3.0 * 3600.0),
            forecast_now: reading(200.0, 300.0),
            ..Sky::default()
        };
        assert_eq!(estimate(NOON, &sky, NOW, 0.02).source, GhiSource::Forecast);

        let sky = Sky {
            satellite: reading(700.0, 3.0 * 3600.0),
            ..Sky::default()
        };
        let a = estimate(NOON, &sky, NOW, 0.02);
        assert_eq!(a.source, GhiSource::ClearSkyGuess);
        assert!((a.ghi - a.clear_sky_ghi * UNKNOWN_CLEAR_SKY_INDEX).abs() < 1e-9);
    }

    #[test]
    fn forecast_drift_shifts_delayed_satellite_value() {
        let sky = Sky {
            satellite: reading(600.0, 1200.0),
            forecast_now: reading(250.0, 60.0),
            forecast_at_satellite_time: Some(550.0),
        };
        let a = estimate(NOON, &sky, NOW, 0.02);
        assert!((a.ghi - 300.0).abs() < 1e-9);
    }

    #[test]
    fn drift_never_goes_negative() {
        let sky = Sky {
            satellite: reading(50.0, 1200.0),
            forecast_now: reading(0.0, 60.0),
            forecast_at_satellite_time: Some(400.0),
        };
        assert_eq!(estimate(NOON, &sky, NOW, 0.02).ghi, 0.0);
    }
}
