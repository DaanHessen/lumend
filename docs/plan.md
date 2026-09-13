# Plan

Implementation plan for the design in [design.md](design.md). Work happens in phases. Each phase ends with passing tests and a commit, so the tree is never left half-broken.

Constraints for every task:

- Rust 2024 edition, `rust-version = "1.90"`, stable toolchain.
- No async runtime. Threads and `std::sync::mpsc`.
- No comments in code unless a function is long, non-obvious and not explained by the rest of its file.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass at the end of every phase.
- No network access outside `sources::sky`.

## Phase 0: skeleton and docs

- [x] Research notes (`docs/research.md`)
- [x] Design (`docs/design.md`)
- [x] This plan
- [x] Cargo manifest, dependency set, `.gitignore`, MIT licence
- [x] `config` module with defaults and TOML loading, tested against an empty file and a partial file

## Phase 1: pure maths

Everything here is deterministic and has no I/O, which makes it the easiest to test thoroughly.

- [x] `perceptual`: `to_p(level, max)` and `to_level(p, max)`. Tests: endpoints map to 0 and 1, round trip within one level, monotonic.
- [x] `solar`: NOAA sun position from UTC time and coordinates. Tests: solar noon elevation in Amsterdam at the June solstice (about 61.1°) and December solstice (about 14.2°), night is negative.
- [x] `solar::clear_sky_ghi`: Haurwitz. Tests: zero below the horizon, about 1,000 W/m² near zenith.
- [x] `location`: ISO 6709 parsing of `zone1970.tab` rows, lookup by zone name, rounding to 0.1°. Tests on inline fixture rows.
- [x] `ambient`: the illuminance estimate with the fallback chain. Tests: night gives the artificial floor, clear noon gives thousands of lux, a stale satellite value is ignored.

## Phase 2: learning

- [x] `features`: `Context` to a 36-entry vector, salted FNV-1a buckets. Tests: missing values encode as documented, the same app always lands in the same bucket, different salts scatter.
- [x] `model::prior`: log-quadratic curve. Tests: the three anchor points, monotonic in lux.
- [x] `model::linear`: conjugate update. Tests: with no data equals the prior; after 40 samples from a known linear function recovers it within 0.02.
- [x] `model::mlp`: forward pass, backprop, Adam. Tests: gradient check against finite differences; learns XOR-like interaction the linear model cannot.
- [x] `model::neighbors`: kernel regression. Tests: exact recall of a repeated point, falls back to the prior far from data.
- [x] `model::ensemble`: exponential weighting. Tests: weight moves to the model that is right; floor keeps every weight above 0.01.
- [x] `model::short_term`: decay and reset. Tests: half-life, reset on a factor-three lux change.
- [x] `store`: JSONL append, load, compaction, model state save and load, corrupt file handling. Tests in a temp directory.
- [x] Simulation test: synthetic user over 30 days, corrections fall and the ensemble beats the prior.

## Phase 3: inputs and outputs

Each source is a thread that owns its connection and sends events. Each gets a small pure parsing function with tests, and the I/O shell around it is kept thin.

- [x] `backlight`: discovery by type, 5 Hz polling, logind `SetBrightness`, sysfs fallback.
- [x] `sources::hyprland`: socket2 reader. Parse `activewindow>>class,title`, `fullscreen>>0|1`, `workspace>>`, `activespecial>>`. Tests on sample lines.
- [x] `sources::power`: `/sys/class/power_supply` scan. Tests on a fake sysfs tree.
- [x] `sources::network`: iwd `Station.State` and `ConnectedNetwork` over the system bus, NetworkManager as fallback.
- [x] `sources::media`: MPRIS players on the session bus, `PlaybackStatus` for players that usually show video.
- [x] `sources::nightlight`: `hyprctl hyprsunset temperature`. Tests on output parsing.
- [x] `sources::sky`: request building and response parsing for both endpoints, backoff. Tests on recorded JSON.
- [x] `sources::idle`: `ext-idle-notify-v1`.
- [x] `sources::screen`: `wlr-screencopy` into shm, grid sampling with an sRGB lookup table. Tests on the sampler with synthetic buffers.

## Phase 4: control

- [x] `controller`: the four states, deadband, debounce, asymmetric ramps, break-moment steps, idle dim, weak confirmations. All transitions tested with a fake clock and scripted events.
- [x] Core loop wiring `Event`s into `Context`, controller and store.
- [x] `ipc`: socket server and the CLI client. Tests for request parsing and a round trip over a real socket.
- [x] `main`: clap subcommands `run` (with `--dry-run`), `status`, `why`, `pause`, `resume`, `forget`.

## Phase 5: packaging

- [x] systemd user unit `lumend.service`, `PartOf=graphical-session.target`.
- [x] Example config `dist/config.toml` with every option commented.
- [x] `PKGBUILD` and `.SRCINFO` for `lumend-git`, following the Arch Rust package guidelines.
- [x] README.

## Phase 6: verification on the target laptop

- [x] Dry run under Hyprland: every source reports, the target follows the prior, no writes.
- [x] CPU and memory use measured over a dry run.
- [x] Live run: press Fn+F7/F8, confirm a single correction is recorded after the settle window.
- [ ] Confirm `lumend why` output makes sense in daylight.
- [ ] Build the package with `makepkg` from the pushed repository.

## Log

Notes from execution go here, newest last.

- Phase 1: the first version of the prior anchors (0.30, 0.60, 1.0) gave a convex curve, which contradicts the comfort study. Moved to 0.35, 0.70, 0.95.
- Phase 2: `serde_json` needs `float_roundtrip`, otherwise MLP weights change in the last bit on save and load.
- Phase 2: the MLP ended up with an output layer that is linear rather than sigmoid, because it predicts a residual.
- Phase 2 simulation (seed 2026): corrections per day went 12, 3, 3, 0, 0, 1, 0, ... and stayed at zero or one. Mean absolute error over the last ten days was 0.092 for the prior and 0.025 for the ensemble. Final weights: prior 0.11, linear 0.45, MLP 0.05, neighbours 0.40. The synthetic user is consistent and has no drift, so real use will be noisier than this.
- Phase 4, controller: 0.02 p/s dimming turned out too fast at the top of the scale (60% to 28% in ten seconds). Default lowered to 0.01 p/s.
- Phase 6, hardware check: the EC applies a written level at once (fifteen reads at 20 ms intervals all returned the new value), so exact matching is enough to tell lumend's own writes from key presses.
- Phase 6, first dry run on the laptop. Four findings:
  - `Europe/Amsterdam` is not in `zone1970.tab` any more (tzdata merged it into Brussels), so the location lookup failed and the irradiance source never started. The lookup now falls back to `zone.tab`.
  - The untrained linear model reported a predictive standard deviation above 1, which pushed the ensemble's uncertainty to 0.33 and the deadband to 0.19 p. In 60 seconds lumend did not move once. The controller now uses the disagreement between members instead, and member variances are capped at the prior's.
  - The prior asked for level 24 at 2 a.m. while the level was 15. The artificial light floor dropped from 80 to 40 lx, the middle of the evening range in the comfort study, which gives about level 19 for a dark terminal.
  - The active app came through as none although Hyprland reports `Alacritty`. Signal events are now logged at debug level to find out why.
- Phase 6, second dry run: location, irradiance, window, fullscreen, power, network, media, night light and screen signals all arrived, and the app was reported correctly. The missing app in the first run did not come back and the cause is unknown, most likely a startup race. The target moved from level 15 to 19 as expected. The newest satellite value was 42 minutes old, so the freshness limit went from 40 to 60 minutes.
- Phase 6, resource use over a 30 second dry run with screen sampling every 3 seconds: 0.03 s user and 0.07 s system CPU (about 0.3% of one core), 21 MB resident, 17 threads, most of them zbus's internal executor.
- Simulations (seed 2026): with a steady user, corrections per day went 13, 4, 2, 1, 3 and then mostly zero, with a last-ten-day error of 0.023 against the prior's 0.102. Four other seeds pass the same checks. When the simulated user switches to 0.12 dimmer on day 20, corrections jump to 27 and 19, drop to 4, 1, 3, and are back at zero from day 25.
- Phase 6, live run: lumend raised the backlight from 14 to 17 in small steps and recognised every one of its own writes. Pressing the keys (17, 12, 7, 12) produced exactly one correction 30 seconds after the last press: "you chose level 12, lumend expected 18 and now predicts 17". The controller then held level 12. A first correction does not change the ensemble weights, because all four experts made the same prediction before it; the short-term offset carries the user's level until a second correction in a similar situation lets the learned experts win weight.
- Phase 5, packaging: the first `makepkg` build failed at link time with undefined `ring_core_*` symbols. makepkg's C LTO flags don't mix with `ring`'s C objects, as the Arch Rust guidelines warn. `options=('!lto')` fixes it; Rust's own LTO from `Cargo.toml` still applies.
