# lumend

Automatic screen brightness for Linux laptops that have no ambient light sensor.

Phones and some laptops adjust brightness from a light sensor. Most Linux laptops either lack one or don't expose it. lumend works around that. It estimates how bright your room is from the sun's position and measured sunlight, looks at what's on your screen and what you're doing, and learns from the times you correct it with the brightness keys.

It was written for an ASUS Zephyrus G16 running Hyprland, and that's where it gets the most signals. The design and the reasons behind it are in [docs/](docs/).

## How it behaves

For the first few days lumend follows a default curve based on published comfort studies: dimmer in a dark room, brighter in daylight, a little lower for white pages than for dark terminals. When the brightness is wrong, fix it with your usual keys. Once you stop pressing for 30 seconds, lumend records the level you settled on, keeps it there for a while, and learns from it.

Changes are meant to go unnoticed. Brightening happens within a few seconds because eyes adapt to more light quickly. Dimming is slow, around 20 seconds for a clearly visible step. Bigger jumps wait until you switch workspace or window, when the whole screen changes anyway. Step away for 90 seconds and it dims a little, then restores the moment you return.

Under the hood four predictors run side by side: the default curve, a Bayesian linear model, a small neural network and a nearest-neighbour memory. Each correction you make scores them, and the ones that predicted well get more say. In a simulated month with a consistent user, corrections dropped from twelve on the first day to almost none after the first week. Real use will be noisier than that.

## Signals

| Signal | Where it comes from |
|---|---|
| Sun position | Computed locally from the time and your location |
| Measured sunlight | Meteosat or Himawari satellite data via Open-Meteo, every 10 minutes |
| Screen content | Sampled every 3 seconds over `wlr-screencopy`, reduced to two numbers |
| Active app, fullscreen, workspace switches | Hyprland's event socket |
| Idle | `ext-idle-notify-v1` |
| AC or battery, battery level | `/sys/class/power_supply` |
| Wi-Fi network | iwd or NetworkManager, stored only as a hashed group number |
| Video playing | MPRIS on the session bus |
| Night light | `hyprctl hyprsunset temperature` |

Missing signals don't stop it. Without Hyprland you lose the window signals; without a Wayland compositor that supports screencopy you lose screen content. The backlight, sun and sky parts work anywhere.

## Privacy

Everything learned stays in `~/.local/state/lumend/`: a list of corrections (numbers only, app and network names are hashed with a random per-install salt) and the model weights. `lumend forget --yes` deletes both.

Screen frames are never written to disk. Each one is read into shared memory, reduced to an average brightness and the share of bright pixels, and overwritten by the next.

The only network traffic is the sunlight request to open-meteo.com, which contains your latitude and longitude rounded to 0.1° (roughly 10 km). It is on by default because it is the one signal that tells a sunny afternoon from an overcast one. To keep lumend fully offline, set this in `~/.config/lumend/config.toml`:

```toml
[sky]
enabled = false
```

lumend sets brightness through systemd-logind, the same unprivileged call desktop environments use. It needs no root, no udev rule and no extra group.

## Install

From the AUR, either the latest release or the development version:

```sh
paru -S lumend        # or: paru -S lumend-git
systemctl --user enable --now lumend
```

Prebuilt x86_64 binaries are attached to each [GitHub release](https://github.com/DaanHessen/lumend/releases).

From source (Rust 1.90 or newer):

```sh
cargo build --release
install -Dm755 target/release/lumend ~/.local/bin/lumend
install -Dm644 dist/lumend.service ~/.config/systemd/user/lumend.service
sed -i "s|/usr/bin/lumend|$HOME/.local/bin/lumend|" ~/.config/systemd/user/lumend.service
systemctl --user daemon-reload
systemctl --user enable --now lumend
```

The service starts with `graphical-session.target`. If your Hyprland session isn't started through uwsm or another systemd-aware launcher, make sure the Wayland variables reach systemd, for example in `hyprland.conf`:

```
exec-once = dbus-update-activation-environment --systemd WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE
```

## Use

Press Fn+F7 and Fn+F8 together to switch lumend off, and together again to switch it back on. Both keys have to go down within about 100 ms of each other, so pressing one after the other never triggers it. A notification says which way it went, and the setting survives a reboot.

For that to work, both brightness keys have to tell lumend about the press. On Omarchy, add this to `~/.config/hypr/bindings.conf`:

```
unbind = , XF86MonBrightnessUp
unbind = , XF86MonBrightnessDown
bindeld = , XF86MonBrightnessUp, Brightness up, exec, ~/.local/bin/lumend key up & omarchy-brightness-display +5%
bindeld = , XF86MonBrightnessDown, Brightness down, exec, ~/.local/bin/lumend key down & omarchy-brightness-display 5%-
```

Replace `omarchy-brightness-display +5%` with whatever your setup already runs for those keys, such as `brightnessctl set +5%`. Redefining the binding instead of adding a second one keeps exactly one action per key press.

```sh
lumend status      # mode, current and target level, what it has learned
lumend why         # the signals and predictions behind the current target
lumend pause 30    # leave the brightness alone for 30 minutes
lumend pause       # ... or until you resume, remembered across reboots
lumend resume
lumend forget --yes
```

To stop it completely:

```sh
systemctl --user disable --now lumend
```

To see what lumend would do without letting it touch anything, stop the service and run `lumend run --dry-run`. It logs every change it would make and saves nothing.

Logs go to the journal: `journalctl --user -u lumend -f`. Set `LUMEND_LOG=debug` in the unit for more detail.

## Configuration

Every option is optional. [dist/config.toml](dist/config.toml) lists them all with their defaults and a line on what each one does. The ones you are most likely to touch:

- `location.latitude` and `location.longitude`, if your timezone's city is far from where you are
- `learning.settle_seconds`, how long to wait after your last key press before learning
- `transitions.dim_rate`, if dimming feels too quick or too slow
- `sky.daylight_factor`, how much outdoor light reaches your desk

## Limits

lumend can't see artificial light. If you switch on a lamp in the evening it has no way to know, and you will need to correct it. Over time it learns your habits at that hour, but a light sensor would always be better.

The irradiance source has no satellite coverage over North America yet; there it falls back to the forecast model, which is less precise about passing clouds.

## Documentation

- [docs/research.md](docs/research.md): what was measured and read before writing code
- [docs/design.md](docs/design.md): architecture, models and controller
- [docs/plan.md](docs/plan.md): implementation phases and a log of what changed on the way

## License

MIT
