use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coordinates {
    pub latitude: f64,
    pub longitude: f64,
}

impl Coordinates {
    pub fn rounded(self) -> Self {
        let round = |v: f64| (v * 10.0).round() / 10.0;
        Self {
            latitude: round(self.latitude),
            longitude: round(self.longitude),
        }
    }
}

const ZONE_TABLES: [&str; 2] = [
    "/usr/share/zoneinfo/zone1970.tab",
    "/usr/share/zoneinfo/zone.tab",
];

pub fn resolve(latitude: Option<f64>, longitude: Option<f64>) -> Option<Coordinates> {
    if let (Some(latitude), Some(longitude)) = (latitude, longitude) {
        return Some(Coordinates {
            latitude,
            longitude,
        });
    }
    let zone = system_timezone()?;
    let tables: Vec<String> = ZONE_TABLES
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect();
    lookup_in_tables(&tables, &zone)
}

pub fn lookup_in_tables(tables: &[String], zone: &str) -> Option<Coordinates> {
    tables.iter().find_map(|table| lookup_zone(table, zone))
}

pub fn system_timezone() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':');
        if tz.contains('/') {
            return Some(tz.to_owned());
        }
    }
    zone_from_link(&std::fs::read_link("/etc/localtime").ok()?)
}

fn zone_from_link(target: &Path) -> Option<String> {
    let text = target.to_str()?;
    let (_, zone) = text.split_once("zoneinfo/")?;
    Some(zone.trim_start_matches("posix/").to_owned())
}

pub fn lookup_zone(table: &str, zone: &str) -> Option<Coordinates> {
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t'))
        .find_map(|mut cols| {
            let coords = cols.nth(1)?;
            (cols.next()? == zone)
                .then(|| parse_iso6709(coords))
                .flatten()
        })
}

fn parse_iso6709(text: &str) -> Option<Coordinates> {
    let split = text[1..].find(['+', '-'])? + 1;
    let (lat, lon) = text.split_at(split);
    Some(Coordinates {
        latitude: parse_angle(lat, 2)?,
        longitude: parse_angle(lon, 3)?,
    })
}

fn parse_angle(text: &str, degree_digits: usize) -> Option<f64> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };
    let digits = &text[1..];
    if digits.len() < degree_digits + 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let part = |range: std::ops::Range<usize>| {
        digits
            .get(range)
            .map_or(Some(0.0), |s| s.parse::<f64>().ok())
    };
    let degrees = part(0..degree_digits)?;
    let minutes = part(degree_digits..degree_digits + 2)?;
    let seconds = if digits.len() >= degree_digits + 4 {
        part(degree_digits + 2..degree_digits + 4)?
    } else {
        0.0
    };
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "# comment line\n\
NL\t+5222+00454\tEurope/Amsterdam\n\
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)\n\
AU\t-3352+15113\tAustralia/Sydney\tNew South Wales (most areas)\n";

    #[test]
    fn parses_minutes_form() {
        let c = lookup_zone(TABLE, "Europe/Amsterdam").unwrap();
        assert!((c.latitude - 52.3667).abs() < 1e-3);
        assert!((c.longitude - 4.9).abs() < 1e-3);
    }

    #[test]
    fn parses_seconds_form_and_negative_longitude() {
        let c = lookup_zone(TABLE, "America/New_York").unwrap();
        assert!((c.latitude - 40.7142).abs() < 1e-3);
        assert!((c.longitude + 74.0064).abs() < 1e-3);
    }

    #[test]
    fn southern_hemisphere() {
        let c = lookup_zone(TABLE, "Australia/Sydney").unwrap();
        assert!(c.latitude < -33.0 && c.longitude > 151.0);
    }

    #[test]
    fn falls_back_to_zone_tab_for_merged_zones() {
        let zone1970 = "BE,LU,NL\t+5050+00420\tEurope/Brussels\n".to_owned();
        let zone_tab = "NL\t+5222+00454\tEurope/Amsterdam\n".to_owned();
        let c = lookup_in_tables(&[zone1970, zone_tab], "Europe/Amsterdam").unwrap();
        assert!((c.latitude - 52.3667).abs() < 1e-3);
    }

    #[test]
    fn unknown_zone_is_none() {
        assert_eq!(lookup_zone(TABLE, "Mars/Olympus"), None);
    }

    #[test]
    fn rounding_to_a_tenth() {
        let c = Coordinates {
            latitude: 52.3667,
            longitude: 4.8958,
        }
        .rounded();
        assert_eq!(
            c,
            Coordinates {
                latitude: 52.4,
                longitude: 4.9
            }
        );
    }

    #[test]
    fn zone_from_localtime_link() {
        assert_eq!(
            zone_from_link(Path::new("/usr/share/zoneinfo/Europe/Amsterdam")).as_deref(),
            Some("Europe/Amsterdam")
        );
        assert_eq!(
            zone_from_link(Path::new("../usr/share/zoneinfo/posix/Asia/Tokyo")).as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(zone_from_link(Path::new("/etc/foo")), None);
    }

    #[test]
    fn config_overrides_timezone() {
        let c = resolve(Some(1.5), Some(2.5)).unwrap();
        assert_eq!(
            c,
            Coordinates {
                latitude: 1.5,
                longitude: 2.5
            }
        );
    }
}
