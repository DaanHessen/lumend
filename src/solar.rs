use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunPosition {
    pub elevation_deg: f64,
    pub azimuth_deg: f64,
}

impl SunPosition {
    pub fn zenith_rad(&self) -> f64 {
        (90.0 - self.elevation_deg).to_radians()
    }
}

/// NOAA solar position equations (the spreadsheet version), accurate to about
/// 0.01 degrees between 1800 and 2100. `unix_seconds` is UTC.
pub fn position(unix_seconds: f64, latitude: f64, longitude: f64) -> SunPosition {
    let julian_day = unix_seconds / 86_400.0 + 2_440_587.5;
    let t = (julian_day - 2_451_545.0) / 36_525.0;

    let mean_longitude = (280.46646 + t * (36000.76983 + t * 0.0003032)).rem_euclid(360.0);
    let mean_anomaly = 357.52911 + t * (35999.05029 - 0.0001537 * t);
    let eccentricity = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
    let m = mean_anomaly.to_radians();
    let center = m.sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + (2.0 * m).sin() * (0.019993 - 0.000101 * t)
        + (3.0 * m).sin() * 0.000289;
    let true_longitude = mean_longitude + center;
    let omega = (125.04 - 1934.136 * t).to_radians();
    let apparent_longitude = (true_longitude - 0.00569 - 0.00478 * omega.sin()).to_radians();

    let mean_obliquity =
        23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
    let obliquity = (mean_obliquity + 0.00256 * omega.cos()).to_radians();
    let declination = (obliquity.sin() * apparent_longitude.sin()).asin();

    let y = (obliquity / 2.0).tan().powi(2);
    let l0 = mean_longitude.to_radians();
    let equation_of_time_min = 4.0
        * (y * (2.0 * l0).sin() - 2.0 * eccentricity * m.sin()
            + 4.0 * eccentricity * y * m.sin() * (2.0 * l0).cos()
            - 0.5 * y * y * (4.0 * l0).sin()
            - 1.25 * eccentricity * eccentricity * (2.0 * m).sin())
        .to_degrees();

    let minutes_utc = (unix_seconds.rem_euclid(86_400.0)) / 60.0;
    let true_solar_time = (minutes_utc + equation_of_time_min + 4.0 * longitude).rem_euclid(1440.0);
    let hour_angle = (true_solar_time / 4.0 - 180.0).to_radians();

    let lat = latitude.to_radians();
    let cos_zenith = (lat.sin() * declination.sin()
        + lat.cos() * declination.cos() * hour_angle.cos())
    .clamp(-1.0, 1.0);
    let zenith = cos_zenith.acos();

    let azimuth = {
        let denom = lat.cos() * zenith.sin();
        if denom.abs() < 1e-9 {
            if lat > 0.0 { 180.0 } else { 0.0 }
        } else {
            let a = ((lat.sin() * cos_zenith - declination.sin()) / denom)
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            if hour_angle > 0.0 {
                (a + 180.0).rem_euclid(360.0)
            } else {
                (540.0 - a).rem_euclid(360.0)
            }
        }
    };

    SunPosition {
        elevation_deg: 90.0 - zenith.to_degrees(),
        azimuth_deg: azimuth,
    }
}

pub fn clear_sky_ghi(sun: SunPosition) -> f64 {
    let cos_z = sun.zenith_rad().cos();
    if cos_z <= 0.0 {
        return 0.0;
    }
    1098.0 * cos_z * (-0.057 / cos_z).exp()
}

pub fn day_angle(local_seconds_of_day: f64) -> f64 {
    2.0 * PI * local_seconds_of_day / 86_400.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMSTERDAM: (f64, f64) = (52.37, 4.89);

    fn unix(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> f64 {
        let days = days_from_civil(y, mo, d);
        (days * 86_400 + h as i64 * 3600 + mi as i64 * 60) as f64
    }

    fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y } as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m as i64 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn max_elevation(y: i32, mo: u32, d: u32) -> f64 {
        (0..24 * 60)
            .map(|m| {
                position(
                    unix(y, mo, d, 0, 0) + m as f64 * 60.0,
                    AMSTERDAM.0,
                    AMSTERDAM.1,
                )
            })
            .map(|s| s.elevation_deg)
            .fold(f64::MIN, f64::max)
    }

    #[test]
    fn noon_elevation_at_solstices() {
        let june = max_elevation(2026, 6, 21);
        let december = max_elevation(2026, 12, 21);
        assert!((june - 61.07).abs() < 0.1, "june {june}");
        assert!((december - 14.19).abs() < 0.1, "december {december}");
    }

    #[test]
    fn midnight_is_below_horizon_and_azimuth_is_south_at_noon() {
        let night = position(unix(2026, 9, 13, 0, 0), AMSTERDAM.0, AMSTERDAM.1);
        assert!(night.elevation_deg < -20.0);
        let noon = position(unix(2026, 9, 13, 11, 40), AMSTERDAM.0, AMSTERDAM.1);
        assert!(
            (noon.azimuth_deg - 180.0).abs() < 5.0,
            "azimuth {}",
            noon.azimuth_deg
        );
    }

    #[test]
    fn equinox_noon_elevation_is_colatitude_on_both_hemispheres() {
        for (lat, lon) in [(40.015, -105.27), (-33.87, 151.21)] {
            let peak = (0..24 * 60)
                .map(|m| position(unix(2026, 3, 20, 0, 0) + m as f64 * 60.0, lat, lon))
                .map(|s| s.elevation_deg)
                .fold(f64::MIN, f64::max);
            assert!(
                (peak - (90.0 - lat.abs())).abs() < 0.5,
                "lat {lat} peak {peak}"
            );
        }
    }

    #[test]
    fn clear_sky_ghi_behaviour() {
        let below = SunPosition {
            elevation_deg: -3.0,
            azimuth_deg: 0.0,
        };
        let zenith = SunPosition {
            elevation_deg: 90.0,
            azimuth_deg: 0.0,
        };
        let low = SunPosition {
            elevation_deg: 10.0,
            azimuth_deg: 0.0,
        };
        assert_eq!(clear_sky_ghi(below), 0.0);
        assert!((clear_sky_ghi(zenith) - 1037.2).abs() < 1.0);
        assert!(clear_sky_ghi(low) > 100.0 && clear_sky_ghi(low) < 200.0);
    }
}
