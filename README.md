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
| `autark-shot` | Screenshots (Shift+Super+3/4/5); screen recording later |
| `autark-nightlight` | Night light: sunset to sunrise through Wayland gamma control |
| `autark-displays` | Displays: layouts remembered per set of monitors, clamshell mode |
| `autark-sandbox` | Sandbox launcher: runs vendor binaries under a per-app Landlock profile |
| `autark-init` | Initramfs init: TPM-plus-PIN LUKS2 unlock through libcryptsetup and tpm2-tss, then hands over to systemd |
| `autark-pkg` | Autark's package manager (Autark-only): signed packages from recipes, with install, remove, upgrade, search and rollback |

`autark-bar` has its first pieces: the Loop logo, the focused app's name, Wi-Fi (iwd, or NetworkManager elsewhere), volume (PipeWire), battery (UPower) and clock. Each monitor gets its own bar, all fed by one watcher per item. The app name needs the compositor's wlr-foreign-toplevel protocol (Wayfire: enable the `foreign-toplevel` plugin). `autark-notifyd` serves `org.freedesktop.Notifications`: banners at the top right, and a centre with Focus (do not disturb) and every kept notification, opened from the bar's button. The centre's Widgets view, built from the design's `Widgets` artboard, has a clock and calendar, batteries from UPower (the laptop's and Bluetooth devices'), resources from `autark-stats` (watched only while shown), Kith's next tasks (the `autark-task@*.timer` user timers) and its suggestion to quit an app that has kept a core busy or drained the battery for two minutes (one `Snapshot()` a minute), the pinned note from Notes, and opt-in weather from Open-Meteo. Edit widgets switches each one off and turns on the desktop layer, which shows the clock and the pinned note with boxes you can tick. `autark-dock` shows pinned apps and running ones after a separator, with magnification and dots under running apps; clicking focuses an app's window (the next one if it has focus) or launches it, and middle-click opens another window. `autark-launcher` stays resident and opens or closes each time it's run (bind Super+Space to `autark-launcher`; start it at login with `--daemon`): apps ranked by name and how often you open them, recent files, and lock, sleep, restart, shut down and log out. `autark-lock` locks every monitor through `ext-session-lock` (Wayfire: enable the `session-lock` plugin) and unlocks with your password through PAM, or with an enrolled fingerprint through fprintd; with `--daemon` it stays running, locks when logind asks (the launcher's Lock, `loginctl lock-session`) and before sleep. It needs a PAM file, `/etc/pam.d/autark-lock` by default, and won't lock without one. `autark-ai` serves `org.autark.AI1`: `Route` turns a sentence into actions from a fixed catalog (volume, brightness, Wi-Fi, Focus, lock, open an app and so on), a question or a search; `Rank` scores items against a query; `Check` rates a shell command's risk, with built-in rules for destructive commands that work even with AI off; and `Ask` streams an answer as signals. All of it runs on Ollama for now, is off until `enabled = true`, and logs every call to `~/.local/state/autark-ai/log`. Jev, chat, memory, tasks and the MCP server come next. `autark-notes` opens a folder of Markdown files as a vault (`autark-notes [folder]`, or `vault` in notes.toml, or `~/Notes`): folders and tags, a note list with pins and full-text search, and an editor that styles Markdown as you type, with `[[links]]` that open on click, backlinks, checklists, autosave, and renames that update every link; a graph view of how notes link; a quick switcher that also creates notes (Ctrl+O, or Ctrl+T for a new tab); tabs (Ctrl+click a link or Ctrl+W to close); and find and replace (Ctrl+F, Ctrl+H). Changes made outside (git, Obsidian) show up live. `autark-stats` serves `org.autark.Stats1`: call `Watch(interval_ms)` and `Updated` signals carry CPU (total and per core), memory, disk and network rates, battery health, and every app's CPU, memory, disk and processes from its cgroup, with its share of the power draw; `Snapshot()` gives one reading, averaged since the previous one. It reads nothing while no one is watching. Power comes from the CPU's RAPL counter through `autark-stats-power`, a root helper started by D-Bus on demand that answers only with averages over at least a second (too coarse for the PLATYPUS side channel that made the counter root-only), otherwise from the battery while discharging. Its systemd unit, D-Bus activation file and policy are in `crates/autark-stats/data/`. The shared types are in `autark_common::stats`. `autark-monitor` is Activity, the task manager, built from the design's `TaskManager` artboard: apps and their processes by CPU, memory, energy impact and disk, the network rate, a ten-minute chart, battery health, a flag on apps busy for minutes, and force quit, which kills an app's whole cgroup. `autark-monitor --force-quit` opens only the force-quit dialog (bind Ctrl+Alt+Esc to it). `autark-files` is Files, built from the design's `FileManager` artboard (`autark-files [folder…]`): icon and list views, an info pane with image and text previews, favourites and mounted disks in the sidebar, new folder, rename, copy, cut and paste, drag to move, Move to Trash with Undo, and the Trash itself (put back, delete, empty), shared with every other app through the freedesktop Trash. Search matches names in the folder and below; with `autark-ai` on, Jev re-ranks the hits by meaning. Thumbnails go in the shared `~/.cache/thumbnails`. The file code it shares with Notes and the widgets is in `autark_common::files`. `autark-pkg` builds the Autark repo's recipes (`scripts/pkg/`) into signed archives and installs only from those: `install` (dependencies first), `remove` (refused while something depends on it), `upgrade`, `rollback` (to the other kept build, held there), `search`, `info`, `files`, `owns`, and `import` for the file lists the phase scripts recorded. Staged recipes build as the unprivileged `autark-build` user into `$DESTDIR`; tracked ones build as root in place and their new files are found by change time. Archives are signed with minisign (`autark-pkg keygen` makes the key pair), so `minisign -V` checks them too. Files are replaced by rename, a changed file in `/etc` is kept with the package's copy beside it as `.autark-new`, and `--root <dir>` runs all of it against a folder instead of `/`. `autark-sandbox run <app>` runs a vendor binary under its profile in `/etc/autark/sandbox/<app>.toml` and refuses an app without one (desktop files use `Exec=autark-sandbox run <app> %U`). Landlock limits it to the system's files plus the profile's read and write lists, the TCP ports it names, and signals and abstract sockets inside the sandbox; no-new-privileges stops setuid binaries lifting that. Because Landlock before Linux 7.1 can't refuse a Unix-socket connect, the launcher also enters an unprivileged user and mount namespace with empty `/run/user/<uid>` and `/tmp`, putting back only the Wayland and PipeWire sockets, so the session bus (and systemd on it) stays out of reach unless the profile says `dbus = true`; on 7.1 Landlock refuses those connects too. `network = "none"`, the default, adds a network namespace. `autark-sandbox check <app>` shows what a profile lets through. Ubuntu's AppArmor blocks unprivileged namespaces, so there only `--no-namespace` (development) runs. `autark-shot` takes screenshots through wlr-screencopy, built into Wayfire: `autark-shot screen` captures every monitor, `autark-shot area` freezes the screen and lets you drag out an area (Space switches to picking a window, which needs the `ipc` and `ipc-rules` plugins), and `autark-shot options` adds the bar from the design's `Screenshot` artboard, with the whole screen, a window or an area, Save to (the Screenshots folder, the clipboard or both), a timer, the pointer, and the thumbnail. Bind Shift+Super+3, 4 and 5 to the three. Pictures go to `~/Pictures/Screenshots` as `Screenshot 2026-10-17 at 19.50.12.png`, and a thumbnail shows at the bottom right for five seconds: click it to open the picture, or drag it into an app. The clipboard goes through wlr-data-control, since a key-bound capture has no window for GTK's clipboard; the process stays until something else is copied. The options and the last area are kept in `~/.local/state/autark/shot.toml`. `autark-nightlight daemon` warms the screen through wlr-gamma-control from sunset to sunrise, worked out on the laptop from the coordinates in `nightlight.toml` (no location service), or between set times (22:00 to 07:00 until coordinates are given), fading over 30 minutes; start it with the session (Wayfire's `[autostart]`). `autark-nightlight on`, `off` and `toggle` override it until the schedule next changes, `status` says where it is, and `reload` rereads the config; Settings and Kith use the same calls on `org.autark.NightLight1`. In daylight it lets go of the gamma tables, so the compositor's own come back. `autark-displays daemon` arranges the monitors through wlr-output-management: each set of connected monitors (told apart by make, model and serial) comes back as it was last set, a set seen for the first time gets every monitor on, side by side, each as it was last set, and closing the lid with another monitor plugged in turns the laptop panel off (the lid comes from UPower). Wayfire resets every output from its own config when one is plugged in, so the daemon puts the remembered layout back; leave outputs out of `wayfire.ini`. `autark-displays list` shows the monitors and their modes, and `autark-displays set eDP-1 --scale 1.5` (or `--mode`, `--position`, `--transform`, `--on`, `--off`) changes one and remembers it; Settings › Display will use the same `org.autark.Displays1`. Layouts are kept in `~/.config/autark/displays.toml`, which can be edited by hand. The other crates are empty skeletons for now. What each must do is specified in the Autark repo's [`docs/specs/`](https://github.com/Seuketchi/autark/tree/main/docs/specs).

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

`autark-pkg` reads `/etc/autark/pkg.toml` (paths as the target sees them; `--root` moves them all):

```toml
# /etc/autark/pkg.toml
recipes = "/usr/src/autark/scripts/pkg"
sources = "/var/cache/autark/sources"
cache = "/var/cache/autark/pkgs"      # signed archives
db = "/var/lib/autark/pkg"            # what's installed
build_user = "autark-build"
keep = 2                              # builds kept per package, for rollback
```

A sandbox profile, `/etc/autark/sandbox/<app>.toml` (root-owned, so an app can't loosen its own):

```toml
description = "Zoom"
exec = "/opt/zoom/zoom"              # its folder is readable automatically
read = ["~/Pictures"]                # read and execute
write = ["~/Downloads", "~/.zoom"]
network = "all"                      # "none" (default), or { connect = [443], bind = [] }
audio = true                         # PipeWire; wayland and gpu default to true, dbus to false
```

Screenshots, `~/.config/autark/shot.toml`:

```toml
folder = "~/Pictures/Screenshots"    # the default
```

Night light, `~/.config/autark/nightlight.toml`:

```toml
schedule = "sun"                     # "sun" (sunset to sunrise), "custom" (from and to) or "off" (by hand only)
latitude = 7.07                      # for "sun"; never leaves the laptop
longitude = 125.61
from = "22:00"                       # used by "custom", and by "sun" until coordinates are set
to = "07:00"
temperature = 4000                   # kelvin at night
fade_minutes = 30
```

## License

Private until the first working release, then public under MIT or Apache-2.0, at your option. The license files get added at that release.
