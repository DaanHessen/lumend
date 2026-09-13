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

- [ ] `backlight`: discovery by type, 5 Hz polling, logind `SetBrightness`, sysfs fallback.
- [ ] `sources::hyprland`: socket2 reader. Parse `activewindow>>class,title`, `fullscreen>>0|1`, `workspace>>`, `activespecial>>`. Tests on sample lines.
- [ ] `sources::power`: `/sys/class/power_supply` scan. Tests on a fake sysfs tree.
- [ ] `sources::network`: iwd `Station.State` and `ConnectedNetwork` over the system bus.
- [ ] `sources::media`: MPRIS players on the session bus, `PlaybackStatus` and whether the track looks like video.
- [ ] `sources::nightlight`: `hyprctl hyprsunset temperature`. Tests on output parsing.
- [ ] `sources::sky`: request building and response parsing for both endpoints, backoff. Tests on recorded JSON.
- [ ] `sources::idle`: `ext-idle-notify-v1`.
- [ ] `sources::screen`: `wlr-screencopy` into shm, grid sampling with an sRGB lookup table. Tests on the sampler with synthetic buffers.

## Phase 4: control

- [ ] `controller`: the four states, deadband, debounce, asymmetric ramps, break-moment steps, idle dim, weak confirmations. All transitions tested with a fake clock and scripted events.
- [ ] Core loop wiring `Event`s into `Context`, controller and store.
- [ ] `ipc`: socket server and the CLI client. Tests for request parsing.
- [ ] `main`: clap subcommands `run`, `status`, `why`, `pause`, `resume`, `forget`.

## Phase 5: packaging

- [ ] systemd user unit `lumend.service`, `PartOf=graphical-session.target`.
- [ ] Example config `lumend.toml` with every option commented.
- [ ] `PKGBUILD` and `.SRCINFO` for `lumend-git`, following the Arch Rust package guidelines.
- [ ] README.

## Phase 6: verification on the target laptop

- [ ] Run for real under Hyprland, check CPU use with `pidstat`.
- [ ] Press Fn+F7/F8, confirm a single correction is recorded after the settle window.
- [ ] Confirm `lumend why` output makes sense in daylight and at night.
- [ ] Build the package with `makepkg` in a clean chroot.

## Log

Notes from execution go here, newest last.

- Phase 1: the first version of the prior anchors (0.30, 0.60, 1.0) gave a convex curve, which contradicts the comfort study. Moved to 0.35, 0.70, 0.95.
- Phase 2: `serde_json` needs `float_roundtrip`, otherwise MLP weights change in the last bit on save and load.
- Phase 2: the MLP ended up with an output layer that is linear rather than sigmoid, because it predicts a residual.
- Phase 2 simulation (seed 2026): corrections per day went 12, 3, 3, 0, 0, 1, 0, ... and stayed at zero or one. Mean absolute error over the last ten days was 0.092 for the prior and 0.025 for the ensemble. Final weights: prior 0.11, linear 0.45, MLP 0.05, neighbours 0.40. The synthetic user is consistent and has no drift, so real use will be noisier than this.
