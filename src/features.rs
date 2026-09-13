use crate::context::{Context, ScreenStats};
use crate::solar;

pub const DIM: usize = 36;
pub const APP_BUCKETS: usize = 16;
pub const NETWORK_BUCKETS: usize = 8;
pub const LUX: usize = 0;
pub const SCREEN_LUMA: usize = 3;
const APP_OFFSET: usize = 12;
const NETWORK_OFFSET: usize = APP_OFFSET + APP_BUCKETS;

pub type Features = [f64; DIM];

const NAMES: [&str; APP_OFFSET] = [
    "room light",
    "sun elevation",
    "clear-sky index",
    "screen luma",
    "bright screen area",
    "time of day (sin)",
    "time of day (cos)",
    "on AC",
    "battery",
    "night light",
    "video playing",
    "fullscreen",
];

const UNKNOWN_SCREEN: ScreenStats = ScreenStats {
    mean_luma: 0.5,
    bright_fraction: 0.3,
};

pub fn name(index: usize) -> String {
    match index {
        i if i < APP_OFFSET => NAMES[i].to_owned(),
        i if i < NETWORK_OFFSET => format!("app group {}", i - APP_OFFSET),
        i => format!("network group {}", i - NETWORK_OFFSET),
    }
}

pub fn lux_to_feature(lux: f64) -> f64 {
    (lux.max(1.0).log10() - 2.0) / 2.0
}

pub fn feature_to_log_lux(value: f64) -> f64 {
    value * 2.0 + 2.0
}

pub fn encode(ctx: &Context, salt: u64) -> Features {
    let mut x = [0.0; DIM];
    x[LUX] = lux_to_feature(ctx.ambient.lux);
    x[1] = ctx.sun_elevation_deg.to_radians().sin();
    x[2] = ctx.ambient.clear_sky_index.unwrap_or(0.6);

    let screen = ctx.screen.unwrap_or(UNKNOWN_SCREEN);
    x[SCREEN_LUMA] = screen.mean_luma.clamp(0.0, 1.0);
    x[4] = screen.bright_fraction.clamp(0.0, 1.0);

    let angle = solar::day_angle(ctx.local_seconds_of_day);
    x[5] = angle.sin();
    x[6] = angle.cos();

    let (on_ac, battery) = match ctx.power {
        Some(p) => (if p.on_ac { 1.0 } else { 0.0 }, p.battery.unwrap_or(1.0)),
        None => (0.5, 1.0),
    };
    x[7] = on_ac;
    x[8] = battery.clamp(0.0, 1.0);

    x[9] = ctx
        .night_light_kelvin
        .map_or(0.0, |k| ((6500.0 - k as f64) / 3500.0).clamp(0.0, 1.0));
    x[10] = if ctx.video_playing { 1.0 } else { 0.0 };
    x[11] = if ctx.fullscreen { 1.0 } else { 0.0 };

    if let Some(app) = &ctx.app {
        x[APP_OFFSET + bucket(salt, &app.to_lowercase(), APP_BUCKETS)] = 1.0;
    }
    if let Some(network) = &ctx.network {
        x[NETWORK_OFFSET + bucket(salt, network, NETWORK_BUCKETS)] = 1.0;
    }
    x
}

pub fn bucket(salt: u64, text: &str, buckets: usize) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in salt.to_le_bytes().iter().chain(text.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash % buckets as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ambient::{Ambient, GhiSource};
    use crate::context::Power;

    fn context() -> Context {
        Context {
            ambient: Ambient {
                lux: 100.0,
                ghi: 0.0,
                clear_sky_ghi: 0.0,
                clear_sky_index: None,
                source: GhiSource::ClearSkyGuess,
            },
            sun_elevation_deg: -10.0,
            local_seconds_of_day: 0.0,
            screen: None,
            power: None,
            night_light_kelvin: None,
            video_playing: false,
            fullscreen: false,
            app: None,
            network: None,
        }
    }

    #[test]
    fn missing_values_use_neutral_encodings() {
        let x = encode(&context(), 1);
        assert_eq!(x[LUX], 0.0);
        assert_eq!(x[2], 0.6);
        assert_eq!(x[SCREEN_LUMA], 0.5);
        assert_eq!((x[5], x[6]), (0.0, 1.0));
        assert_eq!((x[7], x[8]), (0.5, 1.0));
        assert!(x[APP_OFFSET..].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn lux_scale_round_trips() {
        for lux in [10.0, 100.0, 10_000.0] {
            let log = feature_to_log_lux(lux_to_feature(lux));
            assert!((10f64.powf(log) - lux).abs() / lux < 1e-9);
        }
    }

    #[test]
    fn app_and_network_are_one_hot_and_stable() {
        let mut ctx = context();
        ctx.app = Some("Firefox".into());
        ctx.network = Some("/net/connman/iwd/0/4/abc_psk".into());
        let a = encode(&ctx, 42);
        let b = encode(&ctx, 42);
        assert_eq!(a, b);
        assert_eq!(a[APP_OFFSET..NETWORK_OFFSET].iter().sum::<f64>(), 1.0);
        assert_eq!(a[NETWORK_OFFSET..].iter().sum::<f64>(), 1.0);
        ctx.app = Some("firefox".into());
        assert_eq!(encode(&ctx, 42), a, "app classes are case-insensitive");
    }

    #[test]
    fn salt_changes_bucket_assignment() {
        let names: Vec<String> = (0..64).map(|i| format!("app{i}")).collect();
        let moved = names
            .iter()
            .filter(|n| bucket(1, n, APP_BUCKETS) != bucket(2, n, APP_BUCKETS))
            .count();
        assert!(moved > 40, "only {moved} of 64 moved");
    }

    #[test]
    fn night_light_and_power() {
        let mut ctx = context();
        ctx.night_light_kelvin = Some(3000);
        ctx.power = Some(Power {
            on_ac: false,
            battery: Some(0.4),
        });
        let x = encode(&ctx, 0);
        assert_eq!(x[9], 1.0);
        assert_eq!((x[7], x[8]), (0.0, 0.4));
        ctx.night_light_kelvin = Some(6500);
        assert_eq!(encode(&ctx, 0)[9], 0.0);
    }

    #[test]
    fn names_cover_every_index() {
        assert_eq!(name(0), "room light");
        assert_eq!(name(13), "app group 1");
        assert_eq!(name(DIM - 1), "network group 7");
    }
}
