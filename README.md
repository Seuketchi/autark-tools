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

`autark-bar` has its first pieces: the Loop logo, the focused app's name, Wi-Fi (iwd, or NetworkManager elsewhere), volume (PipeWire), battery (UPower) and clock. Each monitor gets its own bar, all fed by one watcher per item. The app name needs the compositor's wlr-foreign-toplevel protocol (Wayfire: enable the `foreign-toplevel` plugin). `autark-notifyd` serves `org.freedesktop.Notifications`: banners at the top right, and a centre with Focus (do not disturb) and every kept notification, opened from the bar's button. The centre's Widgets view, built from the design's `Widgets` artboard, has a clock and calendar, batteries from UPower (the laptop's and Bluetooth devices'), resources from `autark-stats` (watched only while shown), Kith's next tasks (the `autark-task@*.timer` user timers) and its suggestion to quit an app that has kept a core busy or drained the battery for two minutes (one `Snapshot()` a minute), the pinned note from Notes, and opt-in weather from Open-Meteo. Edit widgets switches each one off and turns on the desktop layer, which shows the clock and the pinned note with boxes you can tick. `autark-dock` shows pinned apps and running ones after a separator, with magnification and dots under running apps; clicking focuses an app's window (the next one if it has focus) or launches it, and middle-click opens another window. `autark-launcher` stays resident and opens or closes each time it's run (bind Super+Space to `autark-launcher`; start it at login with `--daemon`): apps ranked by name and how often you open them, recent files, and lock, sleep, restart, shut down and log out. `autark-lock` locks every monitor through `ext-session-lock` (Wayfire: enable the `session-lock` plugin) and unlocks with your password through PAM, or with an enrolled fingerprint through fprintd; with `--daemon` it stays running, locks when logind asks (the launcher's Lock, `loginctl lock-session`) and before sleep. It needs a PAM file, `/etc/pam.d/autark-lock` by default, and won't lock without one. `autark-ai` serves `org.autark.AI1`: `Route` turns a sentence into actions from a fixed catalog (volume, brightness, Wi-Fi, Focus, lock, open an app and so on), a question or a search; `Rank` scores items against a query; `Check` rates a shell command's risk, with built-in rules for destructive commands that work even with AI off; and `Ask` streams an answer as signals. All of it runs on Ollama for now, is off until `enabled = true`, and logs every call to `~/.local/state/autark-ai/log`. Jev, chat, memory, tasks and the MCP server come next. `autark-notes` opens a folder of Markdown files as a vault (`autark-notes [folder]`, or `vault` in notes.toml, or `~/Notes`): folders and tags, a note list with pins and full-text search, and an editor that styles Markdown as you type, with `[[links]]` that open on click, backlinks, checklists, autosave, and renames that update every link; a graph view of how notes link; a quick switcher that also creates notes (Ctrl+O, or Ctrl+T for a new tab); tabs (Ctrl+click a link or Ctrl+W to close); and find and replace (Ctrl+F, Ctrl+H). Changes made outside (git, Obsidian) show up live. `autark-stats` serves `org.autark.Stats1`: call `Watch(interval_ms)` and `Updated` signals carry CPU (total and per core), memory, disk and network rates, battery health, and every app's CPU, memory, disk and processes from its cgroup, with its share of the power draw; `Snapshot()` gives one reading, averaged since the previous one. It reads nothing while no one is watching. Power comes from the CPU's RAPL counter through `autark-stats-power`, a root helper started by D-Bus on demand that answers only with averages over at least a second (too coarse for the PLATYPUS side channel that made the counter root-only), otherwise from the battery while discharging. Its systemd unit, D-Bus activation file and policy are in `crates/autark-stats/data/`. The shared types are in `autark_common::stats`. `autark-monitor` is Activity, the task manager, built from the design's `TaskManager` artboard: apps and their processes by CPU, memory, energy impact and disk, the network rate, a ten-minute chart, battery health, a flag on apps busy for minutes, and force quit, which kills an app's whole cgroup. `autark-monitor --force-quit` opens only the force-quit dialog (bind Ctrl+Alt+Esc to it). The other crates are empty skeletons for now. What each must do is specified in the Autark repo's [`docs/specs/`](https://github.com/Seuketchi/autark/tree/main/docs/specs).

## Where it runs

- **Desktop crates:** any wlroots compositor (Wayfire, sway, Hyprland, labwc, river) with systemd and D-Bus. GNOME and KDE aren't targeted.
- **`autark-init`:** any systemd distro with LUKS2 and a TPM.
- **`autark-pkg`:** Autark only.
- **Configuration:** paths and settings come from config files, never hard-coded to Autark.

## Develop on Ubuntu today

The shell crates can be written and tested on Ubuntu before Autark exists:

```
sudo apt install libgtk-4-dev libgtk4-layer-shell-dev libgtk4-session-lock-dev libpam0g-dev libpipewire-0.3-dev libclang-dev wayfire grim
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

For the dock:

```toml
# ~/.config/autark/dock.toml
size = 52        # 32 to 80
magnify = true
pinned = ["org.autark.Files", "firefox", "foot", "galley", "org.autark.Settings"]
```

And for notifications:

```toml
# ~/.config/autark/notifyd.toml
banner_seconds = 5   # when the app doesn't say
max_banners = 3
suggestions = true   # Kith's suggestions to quit runaway apps
note = "Plans/Kith.md"   # the Pinned note widget; default: the first note pinned in Notes

# Weather stays off until this is set and switched on in Edit widgets.
[weather]
latitude = 52.52
longitude = 13.41
place = "Berlin"
```

For the lock screen:

```toml
# ~/.config/autark/lock.toml
pam_service = "autark-lock"   # the file in /etc/pam.d
lock_on_sleep = true          # with --daemon
fingerprint = true            # if fprintd has a finger enrolled
```

For Notes:

```toml
# ~/.config/autark/notes.toml
vault = "~/Notes"   # otherwise the last folder opened (Ctrl+Shift+O)
```

For AI:

```toml
# ~/.config/autark/ai.toml
enabled = false   # nothing but Check's rules answers until this is true

[ollama]
url = "http://localhost:11434"
model = ""        # no default: choose after measuring on your laptop
timeout_seconds = 120

[features]        # each on once AI is enabled
route = true
rank = true
check = true
ask = true
```

## License

Private until the first working release, then public under MIT or Apache-2.0, at your option. The license files get added at that release.
