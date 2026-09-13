# Research

Notes collected before any code was written. The question was simple: on a laptop with no ambient light sensor, how close can software get to the automatic brightness a phone has, and what would it take to beat the tools that already exist on Linux?

The target machine is an ASUS ROG Zephyrus G16 (GU603ZV) running Arch and Hyprland. Everything here was checked against that machine where it could be.

## What the hardware gives us

There is no light sensor. `/sys/bus/iio` does not exist, there is no Intel Sensor Hub on the PCI bus and no `ACPI0008` device. The only light-adjacent hardware is the webcam, which was ruled out for privacy reasons early on.

The backlight is `nvidia_wmi_ec_backlight`, a firmware-type device with 101 levels (0 to 100). It stays the active backlight even in hybrid GPU mode. Two properties of its driver shaped the design:

1. It never calls `backlight_force_update()`, so pressing Fn+F7 or Fn+F8 produces no uevent. The keys are handled by the embedded controller, and the kernel only finds out when someone reads `actual_brightness`.
2. Reading `actual_brightness` goes to the EC through WMI every time. It sounds expensive but isn't: 200 reads took 21 ms on this machine, so roughly 0.1 ms each.

Polling five times per second therefore costs about 0.05% of one core, and it is the only way to notice the user's own adjustments.

For writing, systemd-logind exposes `SetBrightness(subsystem, name, value)` on the session object. It is unprivileged for the active session, so the daemon needs no udev rule, no group membership and no setuid helper.

The panel is 1920x1200 at 165 Hz. Reviews of the GU603 family report around 500 nits peak and no PWM on the IPS panels. I could not find a measurement of this exact panel's level-to-luminance curve, so the daemon does not assume one (see "Perceptual scale" below).

## How others do it

### Android

Android Pie shipped a personalised brightness model built with DeepMind ([Android Developers Blog, 2018](https://android-developers.googleblog.com/2018/11/getting-screen-brightness-right-for.html)). The public details are thin but three things are stated plainly:

- A manufacturer curve maps ambient light to brightness, and the learned part adjusts on top of it. The model never starts from zero.
- The slider was moved to a logarithmic scale because people perceive brightness logarithmically.
- Training happens on device. After a week, about half the test users made fewer adjustments, and total adjustments dropped by more than 10%.

The AOSP defaults in `frameworks/base/core/res/res/values/config.xml` are more useful than the blog post. Brightening waits for 4 s of stable light, darkening waits 8 s. A user adjustment acts as a short-term correction that expires after 5 minutes (`config_autoBrightnessShortTermModelTimeout = 300000`). Ramps run at 180 units/s when fast and 60 units/s when slow.

### Windows 11

Microsoft's documentation ([Adaptive brightness](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/sensors-adaptive-brightness)) describes a bucketed curve: nine overlapping lux ranges each mapped to one brightness target. The overlap is what gives hysteresis, and it was introduced because the older continuous curve made the screen fluctuate with sensor noise. Two other points from that page: backlight percent should map to luminance exponentially so steps look even, and 0% must still be readable.

### wluma

[wluma](https://github.com/max-baz/wluma) is the closest Linux equivalent. It captures the screen, computes its luma, reads an ALS (or a webcam, or a time schedule) and learns from manual adjustments by storing data points and interpolating between them. It does nothing until it has learned something. Its README is candid that high variance in the user's own choices confuses it, which is what you expect from a nearest-point method with no prior.

### Clight

[Clight](https://github.com/FedeDP/Clight) is broader (gamma, DPMS, dimming, keyboard backlight) and reads light from a webcam or ALS. It does not learn from corrections in the same way.

### CAPED

The most useful paper was [CAPED](https://susmitjha.github.io/papers/cases14.pdf) (Schuchhardt, Jha et al., CASES 2014). Their initial study found that ambient light alone does not explain preferred brightness. Battery level, location (home or work), time and screen content all mattered. Their model is a pool of sub-models combined with the continuous weighted-majority algorithm (Vovk's aggregating algorithm): each sub-model's weight falls with its prediction error, so the pool follows whichever model is currently right. Against stock Android, mean absolute error dropped by 41.9% and reported satisfaction rose by 23.5%.

They left screen content out of the final system because capturing it caused UI lag on 2014 phones. On a 2022 laptop that cost is negligible (measured below), so we can use it.

## Preferred brightness versus ambient light

Studies agree on the shape even when the numbers differ.

- Optimum screen luminance rises with ambient illuminance. Above about 10 lx it follows a concave quadratic in log(illuminance) ([Applied Sciences 11(9) 4108, 2021](https://www.mdpi.com/2076-3417/11/9/4108), 33 participants, evening conditions).
- For comfort in the evening, ambient 13 to 62 lx paired with screen luminance of 21 to 75 cd/m² gave the lowest reported fatigue in the same study.
- Preferred peak luminance scales with the average luminance level of the content roughly as `Lp = k * APL^alpha` (from display-industry patents; the exponent varies by panel). A dark editor wants less backlight than a white web page in the same room.

These give the fixed prior curve the daemon starts from.

## Estimating room light without a sensor

This part decides whether the project works at all. Indoor light has two sources, daylight through windows and artificial light, and we can estimate only the first.

Daylight at the window scales with outdoor global horizontal irradiance (GHI). Converting W/m² to lux uses luminous efficacy, which varies from 21 to 131 lm/W depending on sky and sun ([conversion guide, J. Meas. Eng. 2020](https://www.researchgate.net/publication/347362859_A_conversion_guide_solar_irradiance_and_lux_illuminance)); around 110 lm/W is typical for daylight. A daylight factor of 1 to 3% is common for a desk a few metres from a window. So 500 W/m² outside gives roughly 500 x 110 x 0.02 = 1100 lx indoors, and an overcast 100 W/m² gives 220 lx. That five-fold swing is exactly what a user notices and corrects for.

The sun's position is computable offline. For clear-sky GHI the Haurwitz model is enough: `GHI_clear = 1098 * cos(z) * exp(-0.057 / cos(z))`, where z is the zenith angle. The ratio of measured GHI to clear-sky GHI (the clear-sky index) says how cloudy it is right now.

### Where measured irradiance comes from

The first plan was weather forecast cloud cover. It turned out to be the weaker option.

Open-Meteo runs a [satellite radiation API](https://open-meteo.com/en/docs/satellite-radiation-api) built on geostationary imagery. For Europe and Africa it uses Meteosat Third Generation data processed by DWD at 0.025° (about 2.5 km), every 10 minutes, with a stated delay of around 20 minutes. Himawari-9 covers Asia and Oceania at 10-minute steps. North America has no satellite source there yet. A test request returned 288 ten-minute values for the previous two days, as documented.

Satellite values are measurements, not forecasts, but they lag. The regular forecast API fills the gap with `minutely_15=shortwave_radiation_instant`, which comes from ICON-D2 and AROME in central Europe and HRRR in North America.

The free tier allows 10,000 calls a day for non-commercial use. One satellite call and one forecast call every 10 minutes is 288 a day.

Location defaults to the coordinates of the system timezone from `/usr/share/zoneinfo/zone1970.tab`, which ships with every Arch install. For `Europe/Amsterdam` that is within 30 km of anywhere in the Netherlands, well inside the scale at which cloud cover varies. Coordinates are rounded to 0.1° before any request. GeoClue with beaconDB would be more precise, but it sends nearby Wi-Fi access points to a server, which is a worse trade than a timezone guess.

Artificial light cannot be estimated. A lamp switched on at night is invisible to the daemon. This is the main limit of the whole approach, and the reason corrections will never drop to zero.

## Other signals

| Signal | Source on this machine | Why it matters |
|---|---|---|
| Screen content | `wlr-screencopy` (Hyprland 0.56 also has `ext-image-copy-capture`) | Brightness preference tracks content APL |
| Active app, fullscreen | Hyprland socket2 events | Per-app habits: dark terminal, bright browser, films |
| Media playing | MPRIS over the session bus | Separates watching from reading |
| AC / battery | `/sys/class/power_supply` | CAPED found battery level shifts preference |
| Idle | `ext-idle-notify-v1` | Dim when away; hide changes on return |
| Network | iwd on the system bus (`Station.ConnectedNetwork`) | Stand-in for location, hashed before storage |
| Night light | `hyprctl hyprsunset temperature` | A warm tint already reads dimmer |

A full-resolution screen capture takes about 60 ms with grim, and grim's software downscaler takes 300 ms. Reading every 8th pixel in each direction of the raw buffer samples 36,000 pixels and needs well under a millisecond, so the daemon samples the buffer itself instead of scaling it.

## Human factors for transitions

Eyes adapt to a brighter scene within about a second, but full dark adaptation takes up to 30 minutes. Brightening can be quick. Dimming should be slow enough that the user's eyes follow it without noticing.

Luminance ramps are hard to see when slow: visual channels respond to the size of a luminance change far more than to its rate ([Vision Research 33(5), 1993](https://www.sciencedirect.com/science/article/abs/pii/0042698993900574)). A slow ramp in log space is the least noticeable way to change brightness.

People also miss changes that happen during a visual interruption (change blindness). Switching workspace, switching app, unlocking and returning from idle all redraw most of the screen. The daemon uses those moments for larger steps and ramps slowly the rest of the time.

## Perceptual scale

The panel's level-to-luminance curve is unknown, and EC backlights are usually close to linear in luminance. The daemon therefore works in a perceptual unit `p` in [0, 1]:

```
p = ln(1 + k*f) / ln(1 + k),   f = level / max_level,   k = 30
```

With k = 30, one step of `p` looks about the same anywhere on the scale, matching the Windows guidance that low levels need finer steps. All models predict `p` and all ramps move in `p`. Only the backlight writer converts back to levels.

## Choosing the model

The training data is the user's own corrections. Realistically that is two to ten a day at first and fewer later. With so few labels, a large neural network would fit noise and swing unpredictably between corrections. That rules out the "medium-sized network" idea on its own.

Four families were considered:

- **Fixed prior curve.** Reasonable on day one, never improves.
- **Bayesian linear regression.** Closed-form update, well calibrated with 5 to 50 samples, gives a predictive variance for free. Cannot capture interactions such as "dark app only matters at night".
- **Small MLP (two hidden layers of 16).** Can capture interactions once there are a few hundred samples. Unreliable before that.
- **Kernel nearest neighbour.** Exact on repeated situations, poor at extrapolation. This is roughly what wluma does.
- **Gradient-boosted trees.** Strong on tabular data but piecewise constant, so predictions jump as features cross split points, which shows up as visible brightness steps. It also needs batch retraining. Rejected.

None of these wins at every stage of a user's history, which is the situation CAPED's aggregation handles. The daemon runs the prior, the linear model, the MLP and the neighbour model side by side and combines them with exponentially weighted averaging. Before each new correction is learned, every model is scored on it, and its weight is multiplied by `exp(-eta * loss)`. On day one the prior carries almost all weight. After a few corrections the linear model takes over. The MLP earns weight only if it actually predicts better, which it may never do for some users. The theory gives a regret bound: the ensemble's cumulative loss is close to that of the best single model in hindsight.

The Android short-term correction is kept on top: a correction applies immediately as an offset that decays over 10 minutes or clears when the estimated room light changes by more than a factor of three, while the ensemble learns the same example permanently.

## What was not verified

- The satellite delay during daylight. The API was tested at night, when every value is zero.
- Whether Hyprland 0.56.2 exposes `ext-image-copy-capture-v1`. It likely does (it shipped around 0.54), and the daemon uses `wlr-screencopy` either way.
- The panel's real luminance curve, which would need a meter.
