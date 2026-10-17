# autark-tools

The Rust tools behind [Autark](https://github.com/Seuketchi/autark): the desktop shell, the Kith assistant's daemon, the apps, and the system tools. They live apart from the distro so other Linux systems can use them too.

## Crates

| Crate | What it is |
| --- | --- |
| `autark-common` | Shared code: config loading, D-Bus helpers and Linen theme tokens |
| `autark-bar` | Top bar: app menu, clock, Wi-Fi, battery, volume, notification centre button |
| `autark-dock` | Dock with magnification and running-app indicators |
| `autark-launcher` | Spotlight-style launcher, clipboard history (Super+V) and Kith's entry point |
| `autark-notifyd` | Notification centre and `org.freedesktop.Notifications` service, with widgets |
| `autark-lock` | Lock screen using `ext-session-lock` and PAM |
| `autark-ai` | Kith's local daemon (`org.autark.AI1`): routing, Jev and Ollama, laptop MCP tools, tasks, memory |
| `autark-stats` | System metrics service over D-Bus; polls only while something displays the numbers |
| `autark-monitor` | Task manager: apps and processes with CPU, memory, disk, network and energy impact; force quit |
| `autark-notes` | Notes: Apple Notes surface over plain Markdown vaults, with links, backlinks and graph view |
| `autark-files` | Files app, sharing file code with Notes |
| `autark-shot` | Screenshots and screen recording (Shift+Super+3/4/5) |
| `autark-nightlight` | Night light daemon using Wayland gamma control |
| `autark-displays` | Display manager: monitor layouts and scaling, switching automatically on dock |
| `autark-sandbox` | Sandbox launcher: runs vendor binaries under a per-app Landlock profile |
| `autark-init` | Initramfs init: TPM-plus-PIN LUKS2 unlock through libcryptsetup and tpm2-tss, then hands over to systemd |
| `autark-pkg` | Autark's package manager (Autark-only): signed packages from recipes, with install, remove, upgrade, search and rollback |

`autark-bar` has its first pieces: the Loop logo, the focused app's name, Wi-Fi (iwd, or NetworkManager elsewhere), volume (PipeWire), battery (UPower) and clock. Each monitor gets its own bar, all fed by one watcher per item. The app name needs the compositor's wlr-foreign-toplevel protocol (Wayfire: enable the `foreign-toplevel` plugin). `autark-notifyd` serves `org.freedesktop.Notifications`: banners at the top right, and a centre with Focus (do not disturb) and every kept notification, opened from the bar's button. The other crates are empty skeletons for now. What each must do is specified in the Autark repo's [`docs/specs/`](https://github.com/Seuketchi/autark/tree/main/docs/specs).

## Where it runs

- **Desktop crates:** any wlroots compositor (Wayfire, sway, Hyprland, labwc, river) with systemd and D-Bus. GNOME and KDE aren't targeted.
- **`autark-init`:** any systemd distro with LUKS2 and a TPM.
- **`autark-pkg`:** Autark only.
- **Configuration:** paths and settings come from config files, never hard-coded to Autark.

## Develop on Ubuntu today

The shell crates can be written and tested on Ubuntu before Autark exists:

```
sudo apt install libgtk-4-dev libgtk4-layer-shell-dev libpipewire-0.3-dev libclang-dev wayfire grim
cargo check
```

Run Wayfire in a nested window, then start a crate inside it with `cargo run -p autark-bar`. Without a window, `WLR_BACKENDS=headless wayfire` runs it off-screen and `grim` takes a screenshot.

## Configuration

Each tool reads `~/.config/autark/<tool>.toml`, then `/etc/autark/<tool>.toml`; every field is optional. For the bar:

```toml
# ~/.config/autark/bar.toml
clock_format = "%a %-d %b  %H:%M"   # chrono strftime; add %S for seconds
battery_percent = true
```

And for notifications:

```toml
# ~/.config/autark/notifyd.toml
banner_seconds = 5   # when the app doesn't say
max_banners = 3
```

## License

Private until the first working release, then public under MIT or Apache-2.0, at your option. The license files get added at that release.
