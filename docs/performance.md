# Performance

What the desktop costs when nothing is happening, how to measure it, and what has been found. Measured on a 12-thread
i5-13420H with 23.7 GB, Wayfire on Wayland, 2026-10-23. Re-measure with the commands below instead of guessing.

## What healthy looks like

Idle means no input, no animation on screen and nothing playing.

| Thing | Healthy at idle |
|---|---|
| Each Autark process (bar, dock, notifyd, launcher, switcher, wallpaper) | 0% CPU, every thread idle |
| Wayfire | a few % at most; it follows what the clients draw |
| Frames a client draws | 0 a second |
| Whole desktop, PSS | about 70 to 100 MB |

Wayfire's CPU is mostly other programs' drawing. A client that redraws on every refresh (about 145 times a second on
this laptop's display) puts Wayfire at 10% or more as well.

## How to measure

**Memory and CPU per process and per thread**, over five seconds:

```sh
scripts/perf/desktop.py wayfire autark-bar autark-dock autark-notifyd
```

Read the PSS column, not RSS. RSS counts GTK, Mesa and every other shared library in full for each process; PSS shares
each page among the processes that map it. On 2026-10-23 the same eight processes were 257 MB by RSS and 70 MB by PSS.
The busiest-threads column says which thread works. All of the bar's cost was on its main (drawing) thread.

**Does a client redraw while idle?** Start it with GDK's frame log and count the lines:

```sh
GDK_DEBUG=frames autark-bar 2>frames.log &   # wait 8 seconds, then
grep -c '^frame' frames.log                  # healthy: 0 (a few at startup are fine)
```

A steady stream with `refresh_interval` (about 7 ms here) between frames is a client drawing on every vsync.

**Which part of a program is responsible**: switch parts off one at a time and measure again. For the bar, a temporary
build that only made one item (`BAR_ONLY=<item>` in `item()`) found the cause in five minutes; hiding items in
`bar.toml` did not, because an empty list falls back to the defaults.

**Is it the machine and not Autark?** Look at load and pressure before blaming the desktop:

```sh
cat /proc/loadavg /proc/pressure/cpu /proc/pressure/io
ps -eo pid,pcpu,rss,etime,comm --sort=-pcpu | head
```

A load average above the core count, or I/O pressure `full` above a few percent, means something else is using the
machine. On 2026-10-23 the load was 20 to 27 on 12 cores from `ark` building rocm-device-libs (eight `cc1plus`) and
cargo builds in other sessions, which starved Wayfire and made Alt+Tab and screenshots feel slow. Run long builds with
`nice -n 15 ionice -c 3` so the desktop wins.

## Findings

### The Kith face redrew on every frame (fixed)

`autark-bar` used 12% of a core and woke about 230 times a second while idle, and Wayfire used another 11% to composite
it. Only the Kith item caused it. `Visor::now()` kept its start time in a `Cell<Option<f64>>` with
`start.get().get_or_insert(..)`; `Cell::get` returns a copy, so the start was never stored and the clock read 0.000
for ever. With the clock stuck, the face always believed its eyes were still turning (`now - since < GLANCE`) and ticked
on every frame, and its pulse and blink never moved either. Fixed with `elapsed()` in `crates/autark-common/src/visor.rs`.

| | Before | After |
|---|---|---|
| `autark-bar` CPU at idle | 12% of a core | 0% |
| Frames drawn in 8 s | 939 | 0 |
| Wayfire CPU at idle | 11 to 15% | 2% |

The launcher's Ask Kith and the notification centre's Widgets use the same face, so they have it fixed once rebuilt.

### Baseline memory, 2026-10-23, before the fix

| Process | RSS | PSS | Private | Heap and other anonymous |
|---|---|---|---|---|
| wayfire | 29 MB | 9 MB | 7 MB | 5 MB |
| autark-bar | 36 MB | 9 MB | 5 MB | 3 MB |
| autark-dock | 47 MB | 24 MB | 22 MB | 21 MB |
| autark-terminal | 37 MB | 12 MB | 10 MB | 9 MB |
| autark-switcher | 30 MB | 6 MB | 4 MB | 4 MB |
| autark-notifyd | 29 MB | 6 MB | 4 MB | 3 MB |
| autark-launcher, autark-wallpaper | 24 to 25 MB | 2 MB | 0 MB | 0 MB |
| Total | 257 MB | 70 MB | | |

Memory is not a problem. Most of each process is shared libraries. The dock is the largest private user; it holds a
picture for each window it has seen (400 px wide, about 0.4 MB each).

## Open

- `autark-bar` memory varies between restarts (RSS 36 to 73 MB, PSS 9 to 48 MB). Not yet explained.
- Activity (`autark-stats`, `sample.rs`) reports RSS per process, so it overstates shared libraries. Reading
  `/proc/<pid>/smaps_rollup` for PSS would match what `scripts/perf/desktop.py` shows.
- `autark-shot`'s lag has not been measured.
- Cache misses and CPU wakeups need `perf`, which is not installed.
