use super::Backoff;
use crate::ambient::{Irradiance, Sky};
use crate::events::Event;
use crate::location::Coordinates;
use serde::Deserialize;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SATELLITE_URL: &str = "https://satellite-api.open-meteo.com/v1/archive";
const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(alias = "minutely_15")]
    hourly: Series,
}

#[derive(Debug, Deserialize)]
struct Series {
    time: Vec<f64>,
    shortwave_radiation_instant: Vec<Option<f64>>,
}

pub fn satellite_url(at: Coordinates) -> String {
    format!(
        "{SATELLITE_URL}?latitude={:.1}&longitude={:.1}&hourly=shortwave_radiation_instant\
         &models=satellite_radiation_seamless&temporal_resolution=native&past_days=1&timeformat=unixtime",
        at.latitude, at.longitude
    )
}

pub fn forecast_url(at: Coordinates) -> String {
    format!(
        "{FORECAST_URL}?latitude={:.1}&longitude={:.1}&minutely_15=shortwave_radiation_instant\
         &past_minutely_15=12&forecast_minutely_15=8&timeformat=unixtime",
        at.latitude, at.longitude
    )
}

pub fn parse_series(json: &str) -> Result<Vec<(f64, f64)>, serde_json::Error> {
    let response: Response = serde_json::from_str(json)?;
    let s = response.hourly;
    Ok(s.time
        .into_iter()
        .zip(s.shortwave_radiation_instant)
        .filter_map(|(t, v)| Some((t, v?)))
        .filter(|(_, v)| v.is_finite() && *v >= 0.0)
        .collect())
}

pub fn latest_before(series: &[(f64, f64)], now: f64) -> Option<Irradiance> {
    series
        .iter()
        .filter(|(t, _)| *t <= now)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|&(observed_at, ghi)| Irradiance { ghi, observed_at })
}

pub fn interpolate(series: &[(f64, f64)], at: f64) -> Option<f64> {
    let before = series
        .iter()
        .filter(|(t, _)| *t <= at)
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let after = series
        .iter()
        .filter(|(t, _)| *t >= at)
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    if after.0 == before.0 {
        return Some(before.1);
    }
    let f = (at - before.0) / (after.0 - before.0);
    Some(before.1 + f * (after.1 - before.1))
}

pub fn combine(satellite: &[(f64, f64)], forecast: &[(f64, f64)], now: f64) -> Sky {
    let satellite = latest_before(satellite, now);
    let forecast_now = interpolate(forecast, now).map(|ghi| Irradiance {
        ghi,
        observed_at: now,
    });
    let forecast_at_satellite_time = satellite.and_then(|s| interpolate(forecast, s.observed_at));
    Sky {
        satellite,
        forecast_now,
        forecast_at_satellite_time,
    }
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<(f64, f64)>, Box<dyn std::error::Error>> {
    let body = agent.get(url).call()?.body_mut().read_to_string()?;
    Ok(parse_series(&body)?)
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

pub fn run(at: Coordinates, interval: Duration, tx: Sender<Event>) {
    let at = at.rounded();
    tracing::info!(
        "irradiance requests enabled: every {} min to open-meteo.com with latitude {:.1}, longitude {:.1} (set sky.enabled = false to stop)",
        interval.as_secs() / 60,
        at.latitude,
        at.longitude
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("lumend/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let (satellite_url, forecast_url) = (satellite_url(at), forecast_url(at));
    let mut backoff = Backoff::new(Duration::from_secs(60), Duration::from_secs(3600));
    loop {
        let satellite = fetch(&agent, &satellite_url);
        let forecast = fetch(&agent, &forecast_url);
        if let Err(e) = &satellite {
            tracing::debug!("satellite request failed: {e}");
        }
        if let Err(e) = &forecast {
            tracing::debug!("forecast request failed: {e}");
        }
        let delay = if satellite.is_err() && forecast.is_err() {
            let d = backoff.next_delay();
            tracing::warn!("irradiance unavailable, retrying in {} s", d.as_secs());
            d
        } else {
            backoff.reset();
            interval
        };
        let sky = combine(
            &satellite.unwrap_or_default(),
            &forecast.unwrap_or_default(),
            unix_now(),
        );
        if tx.send(Event::Sky(sky)).is_err() {
            return;
        }
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SATELLITE: &str = r#"{"latitude":52.4,"longitude":4.9,"hourly_units":{"time":"unixtime"},
        "hourly":{"time":[1000,1600,2200,2800],"shortwave_radiation_instant":[100.0,150.0,null,null]}}"#;
    const FORECAST: &str = r#"{"minutely_15":{"time":[900,1800,2700,3600],"shortwave_radiation_instant":[90.0,180.0,270.0,360.0]}}"#;

    #[test]
    fn parses_both_shapes_and_drops_nulls() {
        assert_eq!(
            parse_series(SATELLITE).unwrap(),
            vec![(1000.0, 100.0), (1600.0, 150.0)]
        );
        assert_eq!(parse_series(FORECAST).unwrap().len(), 4);
        assert!(parse_series("{}").is_err());
    }

    #[test]
    fn latest_ignores_future_values() {
        let s = vec![(1000.0, 100.0), (1600.0, 150.0), (9000.0, 1.0)];
        assert_eq!(
            latest_before(&s, 2000.0),
            Some(Irradiance {
                ghi: 150.0,
                observed_at: 1600.0
            })
        );
        assert_eq!(latest_before(&s, 500.0), None);
    }

    #[test]
    fn interpolation_is_linear_and_bounded() {
        let f = parse_series(FORECAST).unwrap();
        assert_eq!(interpolate(&f, 1350.0), Some(135.0));
        assert_eq!(interpolate(&f, 1800.0), Some(180.0));
        assert_eq!(interpolate(&f, 5000.0), None);
    }

    #[test]
    fn combine_links_satellite_and_forecast() {
        let sky = combine(
            &parse_series(SATELLITE).unwrap(),
            &parse_series(FORECAST).unwrap(),
            2700.0,
        );
        assert_eq!(sky.satellite.unwrap().observed_at, 1600.0);
        assert_eq!(sky.forecast_now.unwrap().ghi, 270.0);
        assert_eq!(sky.forecast_at_satellite_time, Some(160.0));
    }

    #[test]
    fn urls_round_coordinates() {
        let at = Coordinates {
            latitude: 52.3667,
            longitude: 4.8958,
        };
        assert!(satellite_url(at).contains("latitude=52.4&longitude=4.9"));
        assert!(forecast_url(at).contains("timeformat=unixtime"));
    }
}
