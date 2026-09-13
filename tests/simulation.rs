//! A synthetic user with a hidden preference lives with the daemon for 30
//! days and corrects it whenever the screen is noticeably off. The test checks
//! the two claims the design rests on: corrections become rarer, and the
//! learned ensemble ends up closer to the user than the fixed prior curve.

use lumend::ambient::{Ambient, GhiSource};
use lumend::context::{Context, ScreenStats};
use lumend::features::{self, Features};
use lumend::model::ensemble::Ensemble;
use lumend::model::rng::Rng;
use lumend::model::{Sample, prior};

const DAYS: usize = 30;
const STEP_MINUTES: usize = 10;
const WAKE_HOUR: f64 = 7.0;
const SLEEP_HOUR: f64 = 23.5;
const TOLERANCE: f64 = 0.07;
const SALT: u64 = 0x5eed;

struct App {
    class: &'static str,
    luma: f64,
    video: bool,
}

const APPS: [App; 4] = [
    App {
        class: "kitty",
        luma: 0.08,
        video: false,
    },
    App {
        class: "firefox",
        luma: 0.85,
        video: false,
    },
    App {
        class: "code",
        luma: 0.2,
        video: false,
    },
    App {
        class: "mpv",
        luma: 0.3,
        video: true,
    },
];

struct Moment {
    x: Features,
    truth: f64,
}

fn sun_elevation(hour: f64) -> f64 {
    if (6.0..20.0).contains(&hour) {
        55.0 * (std::f64::consts::PI * (hour - 6.0) / 14.0).sin()
    } else {
        -15.0
    }
}

fn moment(hour: f64, cloudiness: f64, app: &App) -> Moment {
    let elevation = sun_elevation(hour);
    let clear_ghi = if elevation > 0.0 {
        1000.0 * elevation.to_radians().sin()
    } else {
        0.0
    };
    let ghi = clear_ghi * cloudiness;
    let estimated_lux = (ghi * 110.0 * 0.02).max(80.0);
    let lamp = if hour >= 19.0 { 150.0 } else { 0.0 };
    let true_lux = estimated_lux.max(lamp);

    let ctx = Context {
        ambient: Ambient {
            lux: estimated_lux,
            ghi,
            clear_sky_ghi: clear_ghi,
            clear_sky_index: (clear_ghi > 25.0).then_some(cloudiness),
            source: GhiSource::Satellite,
        },
        sun_elevation_deg: elevation,
        local_seconds_of_day: hour * 3600.0,
        screen: Some(ScreenStats {
            mean_luma: app.luma,
            bright_fraction: app.luma,
        }),
        power: None,
        night_light_kelvin: (hour >= 23.0).then_some(5000),
        video_playing: app.video,
        fullscreen: app.video,
        app: Some(app.class.to_owned()),
        network: Some("home".into()),
    };

    let late_terminal = if app.class == "kitty" && hour >= 20.0 {
        -0.12
    } else {
        0.0
    };
    let film = if app.video { 0.06 } else { 0.0 };
    let truth = (prior::curve(true_lux.log10()) + 0.08 + late_terminal + film).clamp(0.05, 1.0);

    Moment {
        x: features::encode(&ctx, SALT),
        truth,
    }
}

#[test]
fn corrections_fall_and_the_ensemble_beats_the_prior() {
    let mut rng = Rng::new(2026);
    let mut ensemble = Ensemble::new(SALT);
    let mut samples: Vec<Sample> = Vec::new();
    let mut corrections_per_day = Vec::new();
    let (mut prior_error, mut ensemble_error, mut late_steps) = (0.0, 0.0, 0usize);
    let mut app = 0usize;
    let mut clock = 0.0;

    for day in 0..DAYS {
        let base_cloud = rng.range(0.15, 1.0);
        let mut corrections = 0;
        let mut hour = WAKE_HOUR;
        while hour < SLEEP_HOUR {
            if rng.uniform() < 0.25 {
                app = rng.below(APPS.len());
            }
            let cloudiness = (base_cloud + 0.15 * rng.normal()).clamp(0.05, 1.0);
            let m = moment(hour, cloudiness, &APPS[app]);
            let predicted = ensemble.predict(&m.x).prediction.mean;

            if day >= DAYS - 10 {
                prior_error += (prior::mean(&m.x) - m.truth).abs();
                ensemble_error += (predicted - m.truth).abs();
                late_steps += 1;
            }

            if (predicted - m.truth).abs() > TOLERANCE {
                corrections += 1;
                let label = (m.truth + 0.015 * rng.normal()).clamp(0.0, 1.0);
                let sample = Sample::new(clock, &m.x, label, 1.0);
                samples.push(sample.clone());
                ensemble.learn(&sample, &samples);
            }

            hour += STEP_MINUTES as f64 / 60.0;
            clock += STEP_MINUTES as f64 * 60.0;
        }
        clock += (24.0 - (SLEEP_HOUR - WAKE_HOUR)) * 3600.0;
        corrections_per_day.push(corrections);
    }

    let first: usize = corrections_per_day[..5].iter().sum();
    let last: usize = corrections_per_day[DAYS - 5..].iter().sum();
    let prior_mae = prior_error / late_steps as f64;
    let ensemble_mae = ensemble_error / late_steps as f64;
    let weights = ensemble.predict(&[0.0; features::DIM]).weights;

    println!("corrections per day: {corrections_per_day:?}");
    println!("last 10 days MAE: prior {prior_mae:.3}, ensemble {ensemble_mae:.3}");
    println!("final weights: {weights:.3?}");

    assert!(last * 3 < first, "first 5 days {first}, last 5 days {last}");
    assert!(
        ensemble_mae < prior_mae * 0.5,
        "prior {prior_mae}, ensemble {ensemble_mae}"
    );
    assert!(
        weights[0] < 0.2,
        "prior still carries {:.2} of the weight",
        weights[0]
    );
}
