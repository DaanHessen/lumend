# Design

This document describes how lumend is built. The reasons behind most choices are in [research.md](research.md).

## Goals

1. Fewer manual brightness corrections over time, measured by the daemon itself.
2. Changes the user doesn't notice: slow dimming, quick brightening, bigger steps only at natural breaks.
3. Everything learned stays on the machine. The only network traffic is the irradiance request, which can be turned off.
4. One small static binary, no root, no GPU compute, under 1% CPU on average.

Out of scope for now: X11, GNOME and KDE backends, external monitors over DDC, keyboard backlight and colour temperature. The backend traits leave room for the first three.

## Process layout

lumend is a single binary. `lumend run` is the daemon, started by a systemd user unit tied to `graphical-session.target`. The other subcommands (`status`, `why`, `pause`, `resume`, `forget`) talk to the daemon over a Unix socket.

The daemon uses plain threads and one channel. Each input source runs in its own thread and sends `Event`s to the core loop. The core loop owns all state, so there are no locks around the model or the controller.

```
 backlight poller ──┐
 screen sampler   ──┤
 hyprland socket2 ──┤
 idle notifier    ──┤
 power / network  ──┼──> mpsc::Receiver<Event> ──> core loop ──> logind SetBrightness
 media / nightlight─┤                                 │
 sky (Open-Meteo) ──┤                                 ├──> store (samples.jsonl, model.json)
 control socket   ──┤                                 └──> control socket replies
 1 Hz ticker      ──┘
```

Async would bring a runtime and colour every function signature, and nothing here needs thousands of concurrent tasks. zbus is used through its blocking API.

## Modules

| Module | Responsibility |
|---|---|
| `config` | TOML config with defaults for every field |
| `perceptual` | level <-> `p` conversion |
| `solar` | sun position (NOAA equations), Haurwitz clear-sky GHI |
| `location` | coordinates from config or `zone1970.tab` |
| `ambient` | estimated indoor illuminance from sun and sky |
| `context` | latest value of every signal, with timestamps |
| `features` | turns a `Context` into a fixed-length `f32` vector |
| `model::prior` | research curve, no learning |
| `model::linear` | Bayesian linear regression |
| `model::mlp` | two-layer MLP with Adam, trained on stored samples |
| `model::neighbors` | Gaussian-kernel nearest neighbour |
| `model::ensemble` | exponentially weighted aggregation of the above |
| `model::short_term` | decaying offset from the latest correction |
| `store` | append-only sample log and model state on disk |
| `controller` | target selection, hysteresis, ramps, correction detection |
| `backlight` | device discovery, polling, writing through logind |
| `sources::*` | one file per input thread |
| `ipc` | control socket protocol |

## Signals and features

`Context` holds the latest reading of each source and when it arrived. A reading older than its source's staleness limit counts as missing. Missing values are encoded as neutral numbers, never as zero where zero means something.

The feature vector has 36 entries:

| Index | Feature | Encoding |
|---|---|---|
| 0 | estimated indoor lux | `(log10(lux) - 2) / 2` |
| 1 | sun elevation | `sin(elevation)` |
| 2 | clear-sky index | 0 to 1.2, 0.6 when unknown |
| 3 | screen mean luma | linear light, 0 to 1 |
| 4 | screen bright fraction | share of samples with luma > 0.8 |
| 5, 6 | local time of day | `sin`, `cos` of the 24 h angle |
| 7 | on AC power | 0 or 1 |
| 8 | battery level | 0 to 1 |
| 9 | night light strength | `(6500 - K) / 3500`, clamped to 0 to 1 |
| 10 | video playing | 0 or 1 |
| 11 | fullscreen | 0 or 1 |
| 12 to 27 | app class | one-hot over 16 hash buckets |
| 28 to 35 | network | one-hot over 8 hash buckets |

Hashing uses FNV-1a with a per-install random salt stored in `model.json`, so the stored samples reveal neither app names nor network names.

## Estimating room light

```
ghi_clear = haurwitz(zenith)
ghi       = satellite value if fresher than 40 min
          else forecast value if available
          else ghi_clear * 0.6
daylight  = ghi * 110 lm/W * daylight_factor        (daylight_factor = 0.02)
lux       = max(daylight, artificial_floor)          (artificial_floor = 80 lx)
```

The satellite value is shifted forward by the forecast's change over the delay window, so a cloud that passed 20 minutes ago isn't treated as current. The constants are only the prior's starting point. Learned models see the raw sky features and will correct a wrong daylight factor.

## Models

All models implement

```rust
pub trait Predictor {
    fn predict(&self, x: &Features) -> Prediction;   // mean p and variance
    fn learn(&mut self, samples: &[Sample]);
}
```

**Prior.** `p = q(L) + d*(luma - 0.5)`, where `L = log10(lux)` and `q` is the quadratic through three anchors: 10 lx gives p = 0.35, 300 lx gives 0.70 and 10,000 lx gives 0.95. The anchors make `q` concave, as the comfort study found, and put a 30 lx evening room at about 11% backlight, inside the 21 to 75 cd/m² range that study recommends for this panel's roughly 500 nit peak. `d = -0.10`, so a white page gets slightly less backlight than a dark terminal. Variance is fixed at 0.04.

**Linear.** Conjugate Bayesian regression with a Gaussian prior centred on zero, except for the intercept, which starts at the prior's mean. It predicts the residual from the prior curve rather than `p` itself, so with no data it equals the prior. Noise variance 0.005, prior precision 4.

**MLP.** 36 -> 16 -> 16 -> 1, tanh hidden layers, linear output, since it predicts a residual that can be negative. The output layer starts at zero, so an untrained network agrees with the prior exactly. Minibatch AdamW (batch 32, learning rate 0.005, decoupled weight decay 1e-3), warm-started from the current weights after each correction. Each fit makes about 20,000 sample passes, so the epoch count falls as data grows and every fit costs roughly the same few milliseconds.

**Neighbours.** Gaussian kernel on standardised features with bandwidth 0.7, blended toward the prior when the kernel mass is small.

**Ensemble.** Weights start at `[0.85, 0.05, 0.05, 0.05]` for prior, linear, MLP and neighbours. On each correction every model predicts first, then `w_i *= exp(-eta * s * (p_i - label)^2)` with `eta = 20` and `s` the sample weight (1 for a correction, 0.2 for a weak confirmation). With `eta = 20`, a model that is 0.1 closer than the prior on a typical correction overtakes it within five corrections. Weights are then normalised and floored at 0.01 so no model is locked out, and only after that do the models learn. Output variance is the weighted variance of the members plus their weighted own variance.

**Short-term offset.** After a correction, `offset = label - ensemble(x)` after learning. It decays with a 10-minute half-life and resets when estimated lux moves by more than a factor of three from the value at correction time.

## Controller

The controller runs on every tick (1 Hz) and on relevant events.

States: `Auto`, `UserAdjusting`, `Held`, `Paused`.

- **Auto.** Compute target `p*`. Move toward it only when `|p* - p_now|` exceeds a deadband of `0.03 + 0.5 * sqrt(variance)`, so an uncertain model moves less. A brightening target must hold for 4 s, a dimming target for 8 s. Ramps run at 0.15 p/s up and 0.01 p/s down. On the perceptual scale 0.01 p/s changes luminance by about 3.5% per second, slow enough that a 0.2 p dim takes 20 seconds. At a break moment (workspace or window change, idle resume, unlock) the controller may step up to 0.15 p at once.
- **UserAdjusting.** Entered when the polled level differs from the last level lumend wrote by more than one step. Auto adjustment stops. Every further change restarts the settle timer. When the level has stayed put for `settle_seconds` (30 by default), the controller emits a correction with the features captured at the end of the settle window and moves to `Held`.
- **Held.** Keeps the user's level for at least 3 minutes and until the short-term offset has decayed to less than the deadband, unless estimated lux changes by a factor of three.
- **Paused.** Set over the CLI. No writes, no learning.

Idle handling: after `idle_dim_seconds` (90) without input, the controller ramps to `p* - 0.2` at the slow rate. On activity it restores instantly, which counts as a break moment.

Weak confirmations: if 20 minutes pass in `Auto` with the user active and no correction, the current state is stored as a sample with weight 0.2. Weak samples are capped at 30% of the store and are the first to be dropped.

## Backlight

`backlight` picks a device from config or by type preference (`firmware`, then `platform`, then `raw`). It reads `actual_brightness` at 5 Hz on its own thread and sends a `Backlight(level)` event only on change. Writes go through `org.freedesktop.login1.Session.SetBrightness` on the caller's session. If logind refuses, it falls back to writing sysfs directly when that file is writable.

## Screen sampling

A dedicated Wayland connection binds `zwlr_screencopy_manager_v1` and captures the focused output every 3 seconds into a shared-memory buffer. The sampler walks a grid of every 8th pixel, converts sRGB to linear light with a lookup table and reports mean luma and the bright fraction. Nothing is written to disk and the buffer is reused. Capture pauses while idle.

## Sky

A thread requests the satellite and forecast endpoints every 10 minutes with coordinates rounded to 0.1°. It uses `ureq` with a 10 s timeout and exponential backoff after failures, up to one hour. `sky.enabled = false` stops the thread from being created, and the first start logs one line saying what is sent and where.

## Storage

- `$XDG_STATE_HOME/lumend/samples.jsonl`: one line per correction with timestamp, feature vector, label and weight. At most 5,000 lines; older and weak samples go first on compaction.
- `$XDG_STATE_HOME/lumend/model.json`: ensemble weights, MLP weights, hash salt and counters.

`lumend forget` deletes both files. There are no other copies.

## Control protocol

Line-delimited JSON over `$XDG_RUNTIME_DIR/lumend.sock`, permissions 0600.

```
{"cmd":"status"}              -> state, level, target, weights, corrections today
{"cmd":"why"}                 -> per-model predictions and top feature contributions
{"cmd":"pause","minutes":30}  -> ok
{"cmd":"resume"}              -> ok
{"cmd":"forget"}              -> ok
```

## Errors

Sources never stop the daemon. A source that fails logs a warning, reports itself missing and retries with backoff. The daemon exits only if no backlight device exists or the control socket can't be created. If the model file is corrupt it is moved aside and the ensemble starts from the prior; the sample log is replayed to rebuild it.

## Testing

- Unit tests for every pure module: perceptual conversion, solar position against NOAA reference values, Haurwitz, zone1970 parsing, feature encoding, each model, ensemble weighting, controller state transitions driven by a fake clock.
- A simulation test with a synthetic user who has a hidden preference function and corrects when the error exceeds a threshold. It checks that corrections per simulated day fall over 30 days and that the ensemble beats the prior alone.
- Manual verification on the target laptop, recorded in [plan.md](plan.md).
