use crate::ambient::{self, Sky};
use crate::backlight::{self, Writer};
use crate::config::Config;
use crate::context::{Context, Power, ScreenStats};
use crate::controller::{Action, Controller, Mode, Settings};
use crate::events::Event;
use crate::features::{self, Features};
use crate::ipc::{self, Command, Request, Response};
use crate::location::{self, Coordinates};
use crate::model::ensemble::{Breakdown, EXPERTS, Ensemble, EnsembleState};
use crate::model::short_term::ShortTerm;
use crate::model::{Prediction, Sample};
use crate::solar::{self, SunPosition};
use crate::store::{self, Store};
use crate::{paths, perceptual, sources};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TICK: Duration = Duration::from_millis(250);
const SCREEN_STALE_SECONDS: f64 = 20.0;
const WEAK_WEIGHT: f64 = 0.2;
const MODEL_VERSION: u32 = 1;

type Error = Box<dyn std::error::Error>;

#[derive(Debug, Serialize, Deserialize)]
struct ModelFile {
    version: u32,
    salt: u64,
    ensemble: EnsembleState,
}

#[derive(Debug, Default)]
struct Signals {
    screen: Option<(ScreenStats, f64)>,
    app: Option<String>,
    fullscreen: bool,
    video_playing: bool,
    idle: bool,
    power: Option<Power>,
    network: Option<String>,
    night_light: Option<u32>,
    sky: Sky,
}

struct Snapshot {
    context: Context,
    x: Features,
    breakdown: Breakdown,
    offset: f64,
    target: Prediction,
}

struct Core {
    config: Config,
    writer: Writer,
    store: Store,
    samples: Vec<Sample>,
    ensemble: Ensemble,
    salt: u64,
    short_term: ShortTerm,
    controller: Controller,
    signals: Signals,
    coordinates: Option<Coordinates>,
    screen_active: Arc<AtomicBool>,
    last: Option<Snapshot>,
    session_corrections: u32,
    dry_run: bool,
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn local_seconds_of_day() -> f64 {
    let t = jiff::Zoned::now().time();
    f64::from(t.hour()) * 3600.0 + f64::from(t.minute()) * 60.0 + f64::from(t.second())
}

fn fresh_salt() -> u64 {
    let mut bytes = [0u8; 8];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        return unix_now().to_bits() ^ u64::from(std::process::id());
    }
    u64::from_le_bytes(bytes)
}

fn approximate_sun(local_seconds: f64) -> SunPosition {
    let hour = local_seconds / 3600.0;
    SunPosition {
        elevation_deg: 45.0 * (std::f64::consts::PI * (hour - 6.0) / 12.0).sin(),
        azimuth_deg: 180.0,
    }
}

fn load_model(store: &Store, samples: &[Sample]) -> (u64, Ensemble) {
    match store.load_state::<ModelFile>() {
        Some(file) if file.version == MODEL_VERSION => (
            file.salt,
            Ensemble::restore(file.ensemble, file.salt, samples),
        ),
        _ => {
            let salt = fresh_salt();
            let mut ensemble = Ensemble::new(salt);
            if !samples.is_empty() {
                tracing::info!("rebuilding model from {} stored samples", samples.len());
                ensemble.fit(samples);
            }
            (salt, ensemble)
        }
    }
}

pub fn run(config: Config, dry_run: bool) -> Result<(), Error> {
    if dry_run {
        tracing::info!("dry run: the backlight will not be changed and nothing will be saved");
    }
    let device = backlight::discover(
        Path::new(backlight::SYSFS_ROOT),
        config.backlight.device.as_deref(),
    )?;
    tracing::info!("backlight {} with {} levels", device.name, device.max + 1);
    let start_level = device.level()?;
    let writer = Writer::new(device.clone());

    let store = Store::open(paths::state_dir())?;
    let mut samples = store.load_samples()?;
    if store::prune(&mut samples, store::MAX_SAMPLES) && !dry_run {
        store.rewrite_samples(&samples)?;
    }
    let (salt, ensemble) = load_model(&store, &samples);
    tracing::info!(
        "{} samples, {} corrections learned so far",
        samples.len(),
        ensemble.corrections()
    );

    let coordinates = location::resolve(config.location.latitude, config.location.longitude);
    match coordinates {
        Some(c) => tracing::info!("location {:.1}, {:.1}", c.latitude, c.longitude),
        None => tracing::warn!(
            "no location: set [location] in the config; sun position will be approximate"
        ),
    }

    let listener = ipc::bind(&paths::socket_path())?;
    let (tx, rx) = mpsc::channel();
    let screen_active = Arc::new(AtomicBool::new(true));

    sources::spawn("ipc", tx.clone(), move |tx| ipc::serve(listener, tx));
    let hz = config.backlight.poll_hz;
    sources::spawn("backlight", tx.clone(), move |tx| {
        backlight::poll(device, hz, tx)
    });
    sources::spawn("hyprland", tx.clone(), sources::hyprland::run);
    let idle_after = config.idle.dim_after_seconds;
    sources::spawn("idle", tx.clone(), move |tx| {
        sources::idle::run(idle_after, tx)
    });
    sources::spawn("power", tx.clone(), sources::power::run);
    sources::spawn("network", tx.clone(), sources::network::run);
    sources::spawn("media", tx.clone(), sources::media::run);
    sources::spawn("nightlight", tx.clone(), sources::nightlight::run);
    if config.screen.enabled {
        let interval = Duration::from_secs_f64(config.screen.interval_seconds);
        let active = screen_active.clone();
        sources::spawn("screen", tx.clone(), move |tx| {
            sources::screen::run(interval, active, tx)
        });
    }
    match (config.sky.enabled, coordinates) {
        (true, Some(at)) => {
            let interval = Duration::from_secs(config.sky.interval_minutes * 60);
            sources::spawn("sky", tx.clone(), move |tx| {
                sources::sky::run(at, interval, tx)
            });
        }
        (true, None) => tracing::warn!("irradiance disabled because the location is unknown"),
        (false, _) => tracing::info!("irradiance disabled in config"),
    }
    sources::spawn("ticker", tx.clone(), |tx| {
        while tx.send(Event::Tick).is_ok() {
            std::thread::sleep(TICK);
        }
    });
    drop(tx);

    let controller = Controller::new(
        Settings::from_config(&config),
        writer.device().max,
        start_level,
        unix_now(),
    );
    let mut core = Core {
        config,
        writer,
        store,
        samples,
        ensemble,
        salt,
        short_term: ShortTerm::default(),
        controller,
        signals: Signals::default(),
        coordinates,
        screen_active,
        last: None,
        session_corrections: 0,
        dry_run,
    };
    for event in rx {
        core.handle(event);
    }
    Ok(())
}

impl Core {
    fn handle(&mut self, event: Event) {
        let now = unix_now();
        match &event {
            Event::Tick | Event::Command(_) => {}
            Event::Network(n) => tracing::debug!("network changed (known: {})", n.is_some()),
            other => tracing::debug!("{other:?}"),
        }
        match event {
            Event::Tick => self.tick(now),
            Event::Backlight(level) => self.controller.observe_level(level, now),
            Event::Screen(stats) => self.signals.screen = stats.map(|s| (s, now)),
            Event::ActiveApp(app) => self.signals.app = app,
            Event::Fullscreen(f) => self.signals.fullscreen = f,
            Event::BreakMoment => self.controller.break_moment(now),
            Event::Idle(idle) => {
                self.signals.idle = idle;
                self.controller.set_idle(idle, now);
                self.screen_active.store(!idle, Ordering::Relaxed);
            }
            Event::Power(p) => self.signals.power = p,
            Event::Network(n) => self.signals.network = n,
            Event::VideoPlaying(v) => self.signals.video_playing = v,
            Event::NightLight(k) => self.signals.night_light = k,
            Event::Sky(sky) => self.signals.sky = sky,
            Event::Command(command) => self.command(command, now),
        }
    }

    fn context(&self, now: f64) -> Context {
        let local = local_seconds_of_day();
        let sun = self
            .coordinates
            .map(|c| solar::position(now, c.latitude, c.longitude))
            .unwrap_or_else(|| approximate_sun(local));
        let s = &self.signals;
        Context {
            ambient: ambient::estimate(sun, &s.sky, now, self.config.sky.daylight_factor),
            sun_elevation_deg: sun.elevation_deg,
            local_seconds_of_day: local,
            screen: s
                .screen
                .filter(|(_, at)| now - at <= SCREEN_STALE_SECONDS)
                .map(|(stats, _)| stats),
            power: s.power,
            night_light_kelvin: s.night_light,
            video_playing: s.video_playing,
            fullscreen: s.fullscreen,
            app: s.app.clone(),
            network: s.network.clone(),
        }
    }

    fn tick(&mut self, now: f64) {
        let context = self.context(now);
        let x = features::encode(&context, self.salt);
        let breakdown = self.ensemble.predict(&x);
        let offset = self.short_term.value(now, context.ambient.lux);
        let target = Prediction {
            mean: (breakdown.prediction.mean + offset).clamp(0.0, 1.0),
            variance: breakdown.disagreement,
        };
        for action in self.controller.tick(now, target) {
            match action {
                Action::Set(level) if self.dry_run => {
                    tracing::info!("dry run: would set level {level}");
                }
                Action::Set(level) => {
                    if let Err(e) = self.writer.set(level) {
                        tracing::warn!("setting brightness to {level} failed: {e}");
                    }
                }
                Action::Correction(p) => {
                    self.learn(Sample::new(now, &x, p, 1.0), &context, target.mean)
                }
                Action::Confirmation(p) => {
                    self.learn(Sample::new(now, &x, p, WEAK_WEIGHT), &context, target.mean)
                }
            }
        }
        self.last = Some(Snapshot {
            context,
            x,
            breakdown,
            offset,
            target,
        });
    }

    fn learn(&mut self, sample: Sample, context: &Context, predicted: f64) {
        let strong = !sample.is_weak();
        if !self.dry_run
            && let Err(e) = self.store.append(&sample)
        {
            tracing::warn!("could not save sample: {e}");
        }
        self.samples.push(sample.clone());
        if store::prune(&mut self.samples, store::MAX_SAMPLES)
            && !self.dry_run
            && let Err(e) = self.store.rewrite_samples(&self.samples)
        {
            tracing::warn!("could not compact samples: {e}");
        }
        self.ensemble.learn(&sample, &self.samples);
        if !self.dry_run {
            self.save_model();
        }

        if strong {
            self.session_corrections += 1;
            let x = sample.features().unwrap_or([0.0; features::DIM]);
            let learned = self.ensemble.predict(&x).prediction.mean;
            self.short_term
                .set(sample.p - learned, sample.time, context.ambient.lux);
            let max = self.writer.device().max;
            tracing::info!(
                "learned correction: you chose level {}, lumend expected {} and now predicts {}",
                perceptual::to_level(sample.p, max),
                perceptual::to_level(predicted, max),
                perceptual::to_level(learned, max),
            );
        }
    }

    fn save_model(&self) {
        let file = ModelFile {
            version: MODEL_VERSION,
            salt: self.salt,
            ensemble: self.ensemble.state(),
        };
        if let Err(e) = self.store.save_state(&file) {
            tracing::warn!("could not save model: {e}");
        }
    }

    fn command(&mut self, command: Command, now: f64) {
        let response = match command.request {
            Request::Status => Response::ok(self.status()),
            Request::Why => Response::ok(self.why()),
            Request::Pause { minutes } => {
                let until = minutes.map(|m| now + m as f64 * 60.0);
                self.controller.pause(until);
                Response::ok(json!({ "paused_until": until }))
            }
            Request::Resume => {
                self.controller.resume();
                Response::ok(json!({ "mode": self.controller.mode().name() }))
            }
            Request::Forget if self.dry_run => Response::error("forget is disabled in a dry run"),
            Request::Forget => match self.store.forget() {
                Ok(()) => {
                    self.samples.clear();
                    self.salt = fresh_salt();
                    self.ensemble = Ensemble::new(self.salt);
                    self.short_term.clear();
                    self.session_corrections = 0;
                    Response::ok(json!({ "forgotten": true }))
                }
                Err(e) => Response::error(format!("could not delete stored data: {e}")),
            },
        };
        let _ = command.reply.send(response);
    }

    fn status(&self) -> serde_json::Value {
        let max = self.writer.device().max;
        let weights: serde_json::Map<String, serde_json::Value> = EXPERTS
            .iter()
            .zip(self.ensemble.state().weights)
            .map(|(name, w)| (name.to_string(), json!(w)))
            .collect();
        let paused_until = match self.controller.mode() {
            Mode::Paused { until } => until,
            _ => None,
        };
        json!({
            "device": self.writer.device().name,
            "mode": self.controller.mode().name(),
            "paused_until": paused_until,
            "level": self.controller.level(),
            "max_level": max,
            "target_level": self.last.as_ref().map(|s| perceptual::to_level(s.target.mean, max)),
            "uncertainty": self.last.as_ref().map(|s| s.target.variance.sqrt()),
            "corrections_total": self.ensemble.corrections(),
            "corrections_this_session": self.session_corrections,
            "samples": self.samples.len(),
            "weights": weights,
        })
    }

    fn why(&self) -> serde_json::Value {
        let Some(s) = &self.last else {
            return json!({ "ready": false });
        };
        let max = self.writer.device().max;
        let level = |p: f64| perceptual::to_level(p, max);
        let experts: Vec<serde_json::Value> = EXPERTS
            .iter()
            .zip(s.breakdown.experts.iter().zip(s.breakdown.weights))
            .map(|(name, (p, w))| json!({ "name": name, "level": level(p.mean), "weight": w }))
            .collect();
        let mut contributions: Vec<(usize, f64)> = self
            .ensemble
            .linear()
            .contributions(&s.x)
            .into_iter()
            .enumerate()
            .collect();
        contributions.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
        let drivers: Vec<serde_json::Value> = contributions
            .iter()
            .take(5)
            .filter(|(_, v)| v.abs() > 0.005)
            .map(|(i, v)| json!({ "feature": features::name(*i), "shift": v }))
            .collect();
        let c = &s.context;
        json!({
            "ready": true,
            "target_level": level(s.target.mean),
            "short_term_offset": s.offset,
            "experts": experts,
            "drivers": drivers,
            "signals": {
                "room_lux": c.ambient.lux,
                "irradiance_source": format!("{:?}", c.ambient.source),
                "ghi": c.ambient.ghi,
                "clear_sky_ghi": c.ambient.clear_sky_ghi,
                "clear_sky_index": c.ambient.clear_sky_index,
                "sun_elevation": c.sun_elevation_deg,
                "screen_luma": c.screen.map(|x| x.mean_luma),
                "app": c.app,
                "fullscreen": c.fullscreen,
                "video_playing": c.video_playing,
                "on_ac": c.power.map(|p| p.on_ac),
                "battery": c.power.and_then(|p| p.battery),
                "night_light_kelvin": c.night_light_kelvin,
                "known_network": c.network.is_some(),
                "idle": self.signals.idle,
            },
        })
    }
}
