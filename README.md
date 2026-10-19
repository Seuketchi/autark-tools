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
| `autark-accounts` | Accounts: Google, iCloud, Fastmail and other mail, calendar and contact accounts signed in once (`org.autark.Accounts1`); passwords and OAuth refresh tokens live in the keyring, and only Autark's own apps are handed credentials, never Kith |
| `autark-pim` | Mail, calendars, tasks and contacts over open protocols: iCalendar, vCard, CalDAV, CardDAV, IMAP and SMTP, for the Calendar, Tasks, Contacts and Mail apps |
| `autark-calendar` | Calendar: a month grid and day agenda over every account's CalDAV calendars, with an event editor; serves `org.autark.Calendar1` (agenda, create, delete) for Kith while it runs |
| `autark-contacts` | Contacts: people from every account's CardDAV address books, searchable and grouped by letter, with an editor; serves `org.autark.Contacts1` (search, create, delete) for Kith and Mail's address completion while it runs |
| `autark-ai` | Kith's local daemon (`org.autark.AI1`): routing, Jev and Ollama, laptop MCP tools, tasks, memory |
| `autark-stats` | System metrics service over D-Bus; polls only while something displays the numbers |
| `autark-wallpaper` | The desktop background: one of four drawn wallpapers or a picture, on every display |
| `autark-monitor` | Task manager: apps and processes with CPU, memory, disk, network and energy impact; force quit |
| `autark-viewer` | Viewer: pictures with zoom toward the pointer, pan, rotate, and the rest of the folder one key away |
| `autark-player` | Player: video and audio through GStreamer, frames drawn by GTK, with controls that fade while a video plays |
| `autark-calc` | Calculator: type a sum or press keys, with the answer shown as you go |
| `autark-archive` | Archives: look inside a zip or tar, take things out, pack files up through `bsdtar`; extraction never overwrites anything |
| `autark-keys` | Keyboard shortcuts: every key combination Wayfire answers, read from its own config so it is never out of date |
| `autark-clock` | Clock: alarms, a timer and a stopwatch; alarms and timers ring through systemd user timers, with the window closed |
| `autark-hw` | Hardware: every device and its driver, what's missing and why, and the laptop's controls; `autark-hw devices`, `doctor` and `controls` work without the window |
| `autark-hw-helper` | Root helper for `autark-hw`, started by D-Bus on demand: loads a driver for hardware that's here and sets a power profile or charge limit, from a short allowlist, after polkit agrees (see `hardware/README.md`) |
| `autark-notes` | Notes: Apple Notes surface over plain Markdown vaults, with links, backlinks and graph view |
| `autark-files` | Files app, sharing file code with Notes |
| `autark-shot` | Screenshots (Shift+Super+3/4/5); screen recording later |
| `autark-nightlight` | Night light: sunset to sunrise through Wayland gamma control |
| `autark-displays` | Displays: layouts remembered per set of monitors, clamshell mode |
| `autark-settings` | Settings: one pane per area, editing the same config files the tools watch |
| `autark-sandbox` | Sandbox launcher: runs vendor binaries under a per-app Landlock profile |
| `autark-init` | Initramfs init: TPM-plus-PIN LUKS2 unlock through libcryptsetup and tpm2-tss, then hands over to systemd |
| `autark-pkg` | Autark's package manager (Autark-only): signed packages from recipes, with install, remove, upgrade, search and rollback |
| `autark-power` | Idle timers (screens off, then sleep; battery or plugged in) and the low-battery warning |

`autark-bar` has its first pieces: the Loop logo, the focused app's name, Wi-Fi (iwd, or NetworkManager elsewhere), volume (PipeWire), battery (UPower) and clock, with Kith's small Visor before the clock while AI is on in ai.toml: it pulses while Kith thinks or answers, has a dot while a suggestion is open, and opens the Assistant panel when clicked. Each monitor gets its own bar, all fed by one watcher per item. The app name needs the compositor's wlr-foreign-toplevel protocol (Wayfire: enable the `foreign-toplevel` plugin). `autark-notifyd` serves `org.freedesktop.Notifications`: banners at the top right, and a centre with Focus (do not disturb) and every kept notification, opened from the bar's button. Two hints of its own: `x-autark-quiet` keeps a notification in the centre with no banner or chime, and `x-autark-open` maps action keys to URIs that notifyd opens itself when the action is clicked, for senders that have exited by then. The centre's Widgets view, built from the design's `Widgets` artboard, has a clock and calendar, batteries from UPower (the laptop's and Bluetooth devices'), resources from `autark-stats` (watched only while shown), Kith's next tasks (the `autark-task@*.timer` user timers) and its newest suggestion from `autark-ai`, under the same Visor, with its offer, Not now and Don't suggest again, the pinned note from Notes, and opt-in weather from Open-Meteo. Edit widgets switches each one off and turns on the desktop layer, which shows the clock and the pinned note with boxes you can tick. `autark-dock` shows pinned apps and running ones after a separator, with magnification and dots under running apps; clicking focuses an app's window (the next one if it has focus) or launches it, and middle-click opens another window. `autark-launcher` stays resident and opens or closes each time it's run (bind Super+Space to `autark-launcher`; start it at login with `--daemon`): apps ranked by name and how often you open them, recent files, and lock, sleep, restart, shut down and log out. With AI on, results end with an Ask Kith row (Tab jumps to it) that turns the launcher into Kith's view, from the design's `AiLauncher` artboard: the actions what you typed asks for, each with its confidence and ticked only when Kith is sure (0.6 or more), run with Enter through `Act` and marked Done; and the answer to what it asks, streamed in with a chip for each file read, Stop, and Open for the file. The footer says what left the laptop (nothing, for now) and has the `[jev] launcher` switch. A fact Kith proposes to remember shows as a card with Save, Edit and Don't save. A proposed task shows as a New task card, from the design's task card: its name, what it does, when (catching up after sleep), what it reads, where it runs and how it's delivered, with Create task, Edit (name, schedule and inputs) and Cancel; once created it shows the timer and its first run. Follow up (Ctrl+Enter) carries on in the Assistant panel, from the design's `Assistant` artboard, which `autark-launcher --assistant` also opens or closes (bind Super+K to it): docked at the top right, it stays open beside your windows and takes the keyboard only when clicked into. Kith's face there and in Ask Kith is the Visor from the design's `Persona` artboard (`autark_common::visor`, drawn with cairo): it blinks when idle, listens for a moment after each key you type to Kith, looks up with three dots while thinking and moves its mouth while answering, as `autark-ai`'s `State` says; with reduced motion on it holds still, and an idle face draws only its blinks. It shows Kith's one running conversation, Ask Kith's exchanges included, with each message's actions to tick and run, answers as they're written (picked up part way when you follow up mid-answer), the files read, the memory cards, the suggestion cards, Memory · N (which opens Settings › AI) and Clear. `autark-lock` locks every monitor through `ext-session-lock` (Wayfire: enable the `session-lock` plugin) and unlocks with your password through PAM, or with an enrolled fingerprint through fprintd; with `--daemon` it stays running, locks when logind asks (the launcher's Lock, `loginctl lock-session`) and before sleep. It needs a PAM file, `/etc/pam.d/autark-lock` by default, and won't lock without one. Under the clock it shows the notifications kept in the centre, one card per app with how many, and what the newest says only with `notification_text = true`; it reads them from `autark-notifyd`'s `Kept` and follows `KeptChanged` while locked. `autark-ai` serves `org.autark.AI1`: `Route` turns a sentence into the actions it asks for, from a fixed catalog (volume, brightness, Wi-Fi, Focus, lock, open an app and so on), each with a confidence, and the question it asks, with the files that question needs; `Rank` scores items against a query; `Check` rates a shell command's risk, with built-in rules for destructive commands that work even with AI off; and `Ask` streams an answer as signals. `AskFiles` does the same with files for context, and `Stop` ends an answer; each `AnswerPiece` says where in the answer it starts. Kith reads only files you name or that a memory in `memory.toml` points to, only in your home folder and never a dotfile or anything in a dot-folder (checked again once symlinks are followed), at most 8 KB of each (the end of a long one) and 16 KB in all, and says which it read. All of it runs on Ollama for now, is off until `enabled = true`, and logs every call to `~/.local/state/autark-ai/log`. `Act` runs the actions `Route` picked, once you've chosen them: volume through `wpctl`, brightness, lock and sleep through logind, Wi-Fi through iwd, Focus through `autark-notifyd`'s new `SetFocus`, night light through `autark-nightlight`, and apps from their desktop entries, found by name; lock and sleep go last. Three more only a suggestion offers: quitting an app you started (a TERM to each process in its cgroup), opening a file with `gio open`, and opening a pane of Settings. `Models` lists the models pulled into Ollama, and ai.toml is reread when it changes; `local_only` and the `[jev]` switches are kept there. The conversation is `Chat` (a message routed with what was said before, its actions and proposed fact kept as entries), `ChatAnswer` (answered with the conversation and what Kith remembers), `ChatAct`, `Remember` (Save adds the fact to the end of `memory.toml`, keeping the rest as it is), `History`, `ClearHistory` and the `HistoryChanged` signal; `State` (idle, thinking while a sentence is routed or before an answer starts, speaking while one is written) and `StateChanged` are for Kith's face; it's kept a week in `~/.local/state/autark-ai/chat.json`, readable only by you. Kith proposes a fact when you ask it to remember one and when it notices something lasting about you, never a secret, and keeps it only if you save it. A sentence that asks for something on a schedule ("every morning at 8, summarise last night's build log") becomes a task Kith proposes; code, not the model, writes its calendar from a few shapes (every day, weekdays, weekends, a weekday or a day of the month, at a time). `CreateTask` writes it to `~/.local/share/autark-ai/tasks/<id>/task.toml` and makes `autark-task@<id>.timer`, a systemd user timer with `Persistent=true` (so a run missed during sleep happens on waking), whose shared service runs `autark-ai task <id>`; `CancelTask` drops the proposal. Each run reads the task's files under the same rules as Ask Kith and the https pages named in the sentence (a plain GET, no cookies, text only, 8 KB kept), asks Ollama, saves the result as `latest.md` and under `runs/` (the last 30 kept), logs the call, and sends the notification itself, so tasks run with the daemon down. A task only reports: it never runs actions. `[tasks]` in ai.toml sets `max` (20) and `minutes` per run (5). Suggestions start from what code notices: an app keeping a core busy or drawing 5 W on battery for two minutes and the battery at 20% on battery, from one `autark-stats` `Snapshot()` a minute; an `autark-pkg` build ending, from its log folder (inotify); and a Wi-Fi network you've never joined in range while the laptop has been offline for two looks, from iwd. Ollama rates each, given Focus (read from notifyd's new `Focus` property) and what Kith remembers, and writes the card; at 0.5 or more it's shown as an entry in the conversation, a `Suggestion` signal and a quiet notification (no banner, no chime), closed once you answer. `AnswerSuggestion` takes `accept` (the offer runs through `Act`), `not_now` or `never`, which stops that kind and subject (that app, that network; any low battery or build) being suggested again; `NeverSuggested` and `SuggestAgain` list and undo those, and `OpenSuggestions` gives the ones still open. At most 3 an hour, an event rated once keeps quiet for a while (an hour for an app, six for a network), and a suggestion stands for an hour. `[suggestions]` in ai.toml sets `per_hour`, `low_battery` and `build_logs`, and `features.suggest` turns them off; they're kept in `~/.local/state/autark-ai/suggestions.json`. Jev, through `[jev]` in ai.toml (each switch off by default, and `local_only` overrides them), answers `Check` (`terminal`), `Route` (`launcher`: it picks the actions and their level or direction from typed yes/no and score questions, and hands anything that needs written text, such as an app's name, a question beside actions, files, a task or a fact, to Ollama), `Rank` (`files` and `settings`, through `RankFor`) and `Urgent` (`notify`: while Focus is on, `autark-notifyd` asks whether a new notification needs you now and lets it through at 0.6 or more); Ollama answers whatever Jev can't, except `Urgent`, which is Jev only so a notification's text goes nowhere else. `autark-ai mcp` is the MCP server, JSON-RPC over stdin and stdout for an agent to start: read-only tools `read_file` (Kith's file rules and limits), `check_command` (the built-in rules), `memory` and `system_stats`, off until `enabled = true`, each call logged. `[web] search = true` adds `web_search` and `autark-ai`'s `Search(query)`: Firecrawl's `/v2/search`, returning the top five results' titles, addresses and snippets (the query leaves the laptop, so it's off by default and `local_only` overrides it); the key is `FIRECRAWL_API_KEY` or the keyring's (`secret-tool store --label=Firecrawl application autark-ai key firecrawl`). Kith's soul is `~/.config/autark-ai/soul.md`, a Markdown note on who it is and how it sounds, in the manner of Hermes' `SOUL.md`; it's added to the instructions for answers, the conversation and task reports (4 KB at most), never to routing, rating or checking. `[mcp] act = true` adds `run_actions`, the catalog's actions through `Act` with no confirmation (off by default). `autark-notes` opens a folder of Markdown files as a vault (`autark-notes [folder]`, or `vault` in notes.toml, or `~/Notes`): folders and tags, a note list with pins and full-text search, and an editor that styles Markdown as you type, with `[[links]]` that open on click, backlinks, checklists, autosave, and renames that update every link; a graph view of how notes link; a quick switcher that also creates notes (Ctrl+O, or Ctrl+T for a new tab); tabs (Ctrl+click a link or Ctrl+W to close); and find and replace (Ctrl+F, Ctrl+H). Changes made outside (git, Obsidian) show up live. `autark-stats` serves `org.autark.Stats1`: call `Watch(interval_ms)` and `Updated` signals carry CPU (total and per core), memory, disk and network rates, battery health, and every app's CPU, memory, disk and processes from its cgroup, with its share of the power draw; `Snapshot()` gives one reading, averaged since the previous one. It reads nothing while no one is watching. Power comes from the CPU's RAPL counter through `autark-stats-power`, a root helper started by D-Bus on demand that answers only with averages over at least a second (too coarse for the PLATYPUS side channel that made the counter root-only), otherwise from the battery while discharging. Its systemd unit, D-Bus activation file and policy are in `crates/autark-stats/data/`. The shared types are in `autark_common::stats`. `autark-monitor` is Activity, the task manager, built from the design's `TaskManager` artboard: apps and their processes by CPU, memory, energy impact and disk, the network rate, a ten-minute chart, battery health, a flag on apps busy for minutes, and force quit, which kills an app's whole cgroup. `autark-monitor --force-quit` opens only the force-quit dialog (bind Ctrl+Alt+Esc to it). `autark-files` is Files, built from the design's `FileManager` artboard (`autark-files [folder…]`): icon and list views, an info pane with image and text previews, favourites and mounted disks in the sidebar, new folder, rename, copy, cut and paste, drag to move, Move to Trash with Undo, and the Trash itself (put back, delete, empty), shared with every other app through the freedesktop Trash. Search matches names in the folder and below; with `autark-ai` on, Jev re-ranks the hits by meaning. Thumbnails go in the shared `~/.cache/thumbnails`. The file code it shares with Notes and the widgets is in `autark_common::files`. `autark-pkg` builds the Autark repo's recipes (`scripts/pkg/`) into signed archives and installs only from those: `install` (dependencies first), `remove` (refused while something depends on it), `upgrade`, `rollback` (to the other kept build, held there), `search`, `info`, `files`, `owns`, and `import` for the file lists the phase scripts recorded. Each build's log ends with how it went (`== autark-pkg: gcc 15.2.0 built in 812s`, or `failed (exit status: 2)`), the line Kith's suggestions watch for. Staged recipes build as the unprivileged `autark-build` user into `$DESTDIR`; tracked ones build as root in place and their new files are found by change time. Archives are signed with minisign (`autark-pkg keygen` makes the key pair), so `minisign -V` checks them too. Files are replaced by rename, a changed file in `/etc` is kept with the package's copy beside it as `.autark-new`, and `--root <dir>` runs all of it against a folder instead of `/`. `autark-sandbox run <app>` runs a vendor binary under its profile in `/etc/autark/sandbox/<app>.toml` and refuses an app without one (desktop files use `Exec=autark-sandbox run <app> %U`). Landlock limits it to the system's files plus the profile's read and write lists, the TCP ports it names, and signals and abstract sockets inside the sandbox; no-new-privileges stops setuid binaries lifting that. Because Landlock before Linux 7.1 can't refuse a Unix-socket connect, the launcher also enters an unprivileged user and mount namespace with empty `/run/user/<uid>` and `/tmp`, putting back only the Wayland and PipeWire sockets, so the session bus (and systemd on it) stays out of reach unless the profile says `dbus = true`; on 7.1 Landlock refuses those connects too. `network = "none"`, the default, adds a network namespace. `autark-sandbox check <app>` shows what a profile lets through. Ubuntu's AppArmor blocks unprivileged namespaces, so there only `--no-namespace` (development) runs. `autark-shot` takes screenshots through wlr-screencopy, built into Wayfire: `autark-shot screen` captures every monitor, `autark-shot area` freezes the screen and lets you drag out an area (Space switches to picking a window, which needs the `ipc` and `ipc-rules` plugins), and `autark-shot options` adds the bar from the design's `Screenshot` artboard, with the whole screen, a window or an area, Save to (the Screenshots folder, the clipboard or both), a timer, the pointer, and the thumbnail. Bind Shift+Super+3, 4 and 5 to the three. Pictures go to `~/Pictures/Screenshots` as `Screenshot 2026-10-17 at 19.50.12.png`, and a thumbnail shows at the bottom right for five seconds: click it to open the picture, or drag it into an app. The clipboard goes through wlr-data-control, since a key-bound capture has no window for GTK's clipboard; the process stays until something else is copied. The options and the last area are kept in `~/.local/state/autark/shot.toml`. `autark-nightlight daemon` warms the screen through wlr-gamma-control from sunset to sunrise, worked out on the laptop from the coordinates in `nightlight.toml` (no location service), or between set times (22:00 to 07:00 until coordinates are given), fading over 30 minutes; start it with the session (Wayfire's `[autostart]`). `autark-nightlight on`, `off` and `toggle` override it until the schedule next changes, `status` says where it is, and `reload` rereads the config; Settings and Kith use the same calls on `org.autark.NightLight1`. In daylight it lets go of the gamma tables, so the compositor's own come back. `autark-displays daemon` arranges the monitors through wlr-output-management: each set of connected monitors (told apart by make, model and serial) comes back as it was last set, a set seen for the first time gets every monitor on, side by side, each as it was last set, and closing the lid with another monitor plugged in turns the laptop panel off (the lid comes from UPower). Wayfire resets every output from its own config when one is plugged in, so the daemon puts the remembered layout back; leave outputs out of `wayfire.ini`. `autark-displays list` shows the monitors and their modes, and `autark-displays set eDP-1 --scale 1.5` (or `--mode`, `--position`, `--transform`, `--on`, `--off`) changes one and remembers it; Settings › Display uses the same `org.autark.Displays1`. Layouts are kept in `~/.config/autark/displays.toml`, which can be edited by hand. `autark-settings [pane]` is Settings, built from the design's `Settings` artboard: a sidebar of panes with search (by name, or by meaning through `autark-ai`'s `Rank` when AI is on), and panes that edit the tools' own config files in place, keeping comments and keys they don't know, the first change copying `/etc/autark`'s file across. Desktop & Dock sets the dock's size, magnification, dots, position and hiding, and the widgets on the desktop (notifyd watches its `widgets.toml` for that), with a button that opens the centre's Edit widgets. Display: the monitors drawn as arranged, dragged to move them (they snap to each other's edges), each one's resolution, refresh rate, scale, rotation or brightness (through logind), turning one off, and night light's schedule, location, warmth and Turn on now. Appearance: light, dark or auto (dark from sunset to sunrise, as night light works it out, even with night light off), six accents, the wallpaper, text size, font, reduced motion and more contrast, all in `theme.toml`. Every tool loads its stylesheet through `autark_common::theme::install`, which fills in the Linen tokens (light, or the dark side derived from the design's dark tokens) and restyles it live when `theme.toml` or the night flag (`$XDG_RUNTIME_DIR/autark/night`, written by `autark-nightlight`) changes. Settings and night light also write `gtk-3.0` and `gtk-4.0`'s `settings.ini`, so other GTK apps opened afterwards are dark or light and use the font. `autark-wallpaper` puts a background behind everything on each display: Linen, Ridge, Dusk or Daylight, drawn rather than shipped as pictures, or a picture's path, covering the display; start it with the session. Sound: the default output's volume and mute, which output and input to use, the input's volume and a live level meter (listening only while the pane is on screen), and sound effects. The PipeWire code the bar and Settings share is `autark_common::audio`, which also synthesizes the two effects, an alert chime that `autark-notifyd` plays for new notifications (not low-urgency ones, updates, or ones that bring their own sound) and the shutter `autark-shot` plays for each capture; `sound.toml`'s `effects = false` turns both off. Power: the battery's charge, time left and health from UPower, the screen-off and sleep timers on battery and plugged in, the low-battery warning, and the bar's percentage (`bar.toml`, which the bar now watches, clock format too). `autark-power` runs those timers: it learns idleness from the compositor's `ext-idle-notify` (an app's idle inhibitor, such as a playing video, holds it off), turns screens off through `wlr-output-power-management` and back on at a touch, sleeps through logind, takes the battery or plugged-in timers by what UPower says, and sends one low-battery notification per discharge. It also locks the session through logind once the screens have been off for `lock_after` seconds (0, the default, as they go off; -1 never). It watches `power.toml` with inotify (`autark_common::changes`, for daemons); start it with the session. Fingerprint & Lock: the fingerprints fprintd holds, added with the sensor's stages shown as a ring and deleted; unlocking with one and showing notification text on the lock screen (`lock.toml`); how soon it locks after the screens go off (`power.toml`); and changing the password through PAM's `passwd` service, in-process. Using a fingerprint for sudo asks for your password and runs `autark-fingerprint-sudo on` or `off` through sudo; installed in `/usr/libexec/autark/`, it writes `/etc/pam.d/autark-fingerprint`, which sudo's PAM file pulls in with `auth include autark-fingerprint` above its password line, so the password still works too. Keyboard: key repeat rate and delay, what Caps Lock does (Caps Lock, Control or Escape), the layouts from xkb's list (Alt+Shift switches once there are two, through `grp:alt_shift_toggle`), and the desktop's shortcuts (the launcher, the three screenshots, force quit). These are Wayfire's: Settings writes them to `wayfire.ini` (`$WAYFIRE_CONFIG_FILE`, or `~/.config/wayfire.ini`), which Wayfire rereads, and sets input options live over its IPC too. A shortcut lives in `[command]` as `binding_<name>` and `command_<name>`, found by its command, so ones written by hand are kept; while it waits for new keys, Settings holds off the compositor's own shortcuts so their keys can be taken. Wi-Fi, through iwd: turning it on and off, the network it's on with its address and security and Disconnect, the known networks (joining automatically or not, Forget, Join when one is in range), the others in range with the password asked in the row, and joining a hidden network. It reads iwd whole with `GetManagedObjects`, follows its signals (and iwd starting or stopping), scans while the pane is shown, and answers iwd's passphrase requests with its own agent; iwd asks the agent of whoever called `Connect`, so the bar's agent and Settings' don't clash. Enterprise (802.1X) networks need a provisioning file in `/var/lib/iwd`. Bluetooth, through BlueZ: turning it on and off, the paired devices (Connect, Disconnect, Forget, and the battery of ones that report it), and the devices nearby with Pair, the passkey to compare shown in the row with Confirm and Cancel; a paired device is trusted and connected at once. While the pane is on screen the laptop searches and can be found under its name. Like Wi-Fi it reads BlueZ whole and follows its signals, and pairs through its own agent, which BlueZ asks because Settings called `Pair`. Connected shows in the theme's new `good` token, a muted green. AI: the switch that turns `autark-ai` on, local-only mode, the Ollama model (from `Models`), and which features Jev answers (`ai.toml`); Jev's API key goes to the Secret Service keyring (oo7-daemon, or GNOME Keyring elsewhere), spoken to directly over D-Bus, and is never read back. Tasks lists Kith's `autark-task@<id>.timer` user timers, and any task folder in `~/.local/share/autark-ai/tasks/`, with Run now, a switch that starts and enables the timer or stops and disables it, and Delete, which also removes the folder and the timer's unit file. Suggestions has their switch and what you said never to suggest again, each with Suggest again. Memory shows what the assistant remembers, `~/.config/autark-ai/memory.toml` (a `[[fact]]` with `text` and `added` for each), to correct or forget, keeping the file's comments. The last five calls from the call log are shown, with a button that opens it. About: the system's name from `/etc/os-release` under the Loop, and what it runs on: the processor, graphics (named from `pci.ids` when it's there), memory and the kinds of swap, the disk the root is on with its filesystem and whether it's encrypted, the kernel, the compositor and the shell's version. The other crates are empty skeletons for now. What each must do is specified in the Autark repo's [`docs/specs/`](https://github.com/Seuketchi/autark/tree/main/docs/specs).

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
dots = true      # under (or beside) running apps
position = "bottom"   # "bottom", "left" or "right"
autohide = false # hidden until the pointer reaches its edge
pinned = ["org.autark.Files", "firefox", "foot", "galley", "org.autark.Settings"]
```

The dock, like the other tools, watches its file: a change applies as soon as it's saved.

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
