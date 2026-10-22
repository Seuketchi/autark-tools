//! Reading the counters: `/proc` for the system, cgroups for your apps, sysfs for the battery
//! and the CPU's energy counter. Each sample turns the change since the last one into rates.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use autark_common::stats::{App, Battery, Cpu, Memory, Power, Process, Rate, Snapshot};

/// `/proc` reports CPU time in these ticks on every architecture Autark runs on.
const TICKS_PER_SECOND: f64 = 100.0;
const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    busy: u64,
    total: u64,
}

#[derive(Clone, Copy, Default)]
struct AppCounters {
    usage_usec: u64,
    disk: Rate,
}

struct Previous {
    at: Instant,
    cpu: Vec<CpuTimes>,
    disk: Rate,
    network: Rate,
    energy_uj: Option<u64>,
    apps: HashMap<String, AppCounters>,
    processes: HashMap<u32, u64>,
}

pub struct Sampler {
    uid: u32,
    previous: Option<Previous>,
    /// Names found for cgroups, since looking up desktop files reads the disk.
    names: HashMap<String, (String, Option<String>)>,
    /// The same for programs in the login session, by the name of one of their processes.
    commands: HashMap<String, Option<(String, String)>>,
}

impl Sampler {
    pub fn new() -> Self {
        let uid = std::fs::metadata("/proc/self").map(|meta| std::os::unix::fs::MetadataExt::uid(&meta)).unwrap_or(0);
        Self { uid, previous: None, names: HashMap::new(), commands: HashMap::new() }
    }

    /// How long ago the last sample was taken.
    pub fn age(&self) -> Option<std::time::Duration> {
        self.previous.as_ref().map(|previous| previous.at.elapsed())
    }

    /// One reading. `helper_watts` is the root helper's answer, used when we can't read the
    /// RAPL counter ourselves.
    pub fn sample(&mut self, helper_watts: Option<f64>) -> Snapshot {
        let at = Instant::now();
        let cpu = read("/proc/stat").map(|text| parse_stat(&text)).unwrap_or_default();
        let memory = read("/proc/meminfo").map(|text| parse_meminfo(&text)).unwrap_or_default();
        let disk = read("/proc/diskstats").map(|text| parse_diskstats(&text)).unwrap_or_default();
        let network = read("/proc/net/dev").map(|text| parse_net_dev(&text, |name| Path::new("/sys/class/net").join(name).join("device").exists())).unwrap_or_default();
        let energy_uj = read(RAPL).and_then(|text| text.trim().parse().ok());
        let battery = battery();
        let cgroups = self.cgroups();

        let previous = self.previous.take();
        let seconds = previous.as_ref().map_or(0.0, |previous| at.duration_since(previous.at).as_secs_f64());
        let per_second = |now: u64, then: u64| if seconds > 0.0 { (now.saturating_sub(then) as f64 / seconds) as u64 } else { 0 };
        let rate = |now: Rate, then: Option<Rate>| then.map_or(Rate::default(), |then| Rate { read: per_second(now.read, then.read), write: per_second(now.write, then.write) });

        let percent = |now: CpuTimes, then: CpuTimes| {
            let total = now.total.saturating_sub(then.total);
            if total == 0 { 0.0 } else { now.busy.saturating_sub(then.busy) as f64 * 100.0 / total as f64 }
        };
        let cpu_percent: Vec<f64> = match &previous {
            Some(previous) => cpu.iter().zip(&previous.cpu).map(|(&now, &then)| percent(now, then)).collect(),
            None => vec![0.0; cpu.len()],
        };
        // All CPU time spent busy since the last sample, in microseconds, to share power out.
        let busy_usec = match (&previous, cpu.first()) {
            (Some(previous), Some(now)) => previous.cpu.first().map_or(0.0, |then| now.busy.saturating_sub(then.busy) as f64 * 1e6 / TICKS_PER_SECOND),
            _ => 0.0,
        };

        let power = match (energy_uj, previous.as_ref().and_then(|previous| previous.energy_uj)) {
            (Some(now), Some(then)) if seconds > 0.0 => {
                let range = read("/sys/class/powercap/intel-rapl:0/max_energy_range_uj").and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(u64::MAX);
                let used = if now >= then { now - then } else { range - then + now };
                Some(Power { watts: used as f64 / 1e6 / seconds, source: "rapl".to_owned() })
            }
            _ => helper_watts.map(|watts| Power { watts, source: "rapl".to_owned() }).or_else(|| {
                battery.as_ref().filter(|battery| battery.status == "Discharging").and_then(|battery| battery.watts).map(|watts| Power { watts, source: "battery".to_owned() })
            }),
        };

        let mut apps: Vec<App> = Vec::new();
        let mut app_counters = HashMap::new();
        let mut process_ticks = HashMap::new();
        for cgroup in cgroups {
            let then = previous.as_ref().and_then(|previous| previous.apps.get(&cgroup.id)).copied();
            let usage = then.map_or(0, |then| cgroup.counters.usage_usec.saturating_sub(then.usage_usec));
            let cpu = if seconds > 0.0 && then.is_some() { usage as f64 / 1e4 / seconds } else { 0.0 };
            let watts = power.as_ref().filter(|_| busy_usec > 0.0).map(|power| power.watts * (usage as f64 / busy_usec).min(1.0));
            let mut processes = Vec::new();
            for (pid, name, ticks, rss, _) in cgroup.processes {
                let cpu = match previous.as_ref().and_then(|previous| previous.processes.get(&pid)) {
                    Some(&then) if seconds > 0.0 => ticks.saturating_sub(then) as f64 * 100.0 / TICKS_PER_SECOND / seconds,
                    _ => 0.0,
                };
                process_ticks.insert(pid, ticks);
                processes.push(Process { pid, name, cpu, memory: rss });
            }
            processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));
            app_counters.insert(cgroup.id.clone(), cgroup.counters);
            let app = App {
                id: cgroup.id,
                name: cgroup.name,
                desktop_id: cgroup.desktop_id,
                system: cgroup.system,
                cpu,
                memory: cgroup.memory,
                disk: rate(cgroup.counters.disk, then.map(|then| then.disk)),
                watts,
                processes,
            };
            // One app can run in several cgroups (a browser and its helpers); show it once.
            match apps.iter_mut().find(|other| other.desktop_id.is_some() && other.desktop_id == app.desktop_id) {
                Some(other) => {
                    other.cpu += app.cpu;
                    other.memory += app.memory;
                    other.disk.read += app.disk.read;
                    other.disk.write += app.disk.write;
                    other.watts = match (other.watts, app.watts) {
                        (Some(a), Some(b)) => Some(a + b),
                        (a, b) => a.or(b),
                    };
                    other.processes.extend(app.processes);
                }
                None => apps.push(app),
            }
        }
        apps.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)));

        self.previous = Some(Previous { at, cpu: cpu.clone(), disk, network, energy_uj, apps: app_counters, processes: process_ticks });
        Snapshot {
            time: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64),
            cpu: Cpu { total: cpu_percent.first().copied().unwrap_or(0.0), cores: cpu_percent.iter().skip(1).copied().collect() },
            memory,
            disk: rate(disk, previous.as_ref().map(|previous| previous.disk)),
            network: rate(network, previous.as_ref().map(|previous| previous.network)),
            power,
            battery,
            apps,
        }
    }

    /// Every cgroup of yours with processes in it: apps and session services, and the programs
    /// that share the login session's cgroup, each on its own.
    fn cgroups(&mut self) -> Vec<Cgroup> {
        let root = PathBuf::from(format!("/sys/fs/cgroup/user.slice/user-{0}.slice", self.uid));
        let mut leaves = Vec::new();
        collect_leaves(&root, &mut leaves);
        leaves
            .into_iter()
            .filter_map(|dir| {
                let pids: Vec<u32> = read(dir.join("cgroup.procs"))?.lines().filter_map(|line| line.trim().parse().ok()).collect();
                if pids.is_empty() {
                    return None;
                }
                let id = dir.strip_prefix("/sys/fs/cgroup").ok()?.to_string_lossy().into_owned();
                let leaf = dir.file_name()?.to_string_lossy().into_owned();
                let processes: Vec<Proc> = pids.into_iter().filter_map(process).collect();
                // Everything started from a launcher or a key binding lives here, so one entry for the cgroup
                // would be one app made of every program on the desktop, and force quit couldn't tell them apart.
                if leaf.starts_with("session-") && leaf.ends_with(".scope") {
                    return Some(self.session_programs(&id, processes));
                }
                let (name, desktop_id) = self.names.entry(id.clone()).or_insert_with(|| name_for(&leaf)).clone();
                let usage_usec = read(dir.join("cpu.stat")).and_then(|text| parse_cpu_stat(&text)).unwrap_or(0);
                let memory = read(dir.join("memory.current")).and_then(|text| text.trim().parse().ok()).unwrap_or(0);
                let disk = read(dir.join("io.stat")).map(|text| parse_io_stat(&text)).unwrap_or_default();
                // Terminal tabs, one-off jobs and the like get random unit names; the process that started them says more.
                let name = match name.strip_suffix(GENERIC) {
                    Some(_) => {
                        let known: Vec<u32> = processes.iter().map(|(pid, ..)| *pid).collect();
                        let head = processes.iter().filter(|(_, _, _, _, parent)| !known.contains(parent)).min_by_key(|(pid, ..)| *pid).or(processes.first());
                        head.map_or(name.clone(), |(pid, comm, ..)| job_name(*pid, comm))
                    }
                    None => name,
                };
                Some(vec![Cgroup { id, name, desktop_id, system: false, counters: AppCounters { usage_usec, disk }, memory, processes }])
            })
            .flatten()
            .collect()
    }

    /// The login session's processes as the programs they belong to. A cgroup has no per-program
    /// counters, so these are added up from the processes.
    fn session_programs(&mut self, id: &str, processes: Vec<Proc>) -> Vec<Cgroup> {
        let parents: Vec<(u32, u32)> = processes.iter().map(|&(pid, _, _, _, parent)| (pid, parent)).collect();
        split_trees(&parents)
            .into_iter()
            .map(|(head, pids)| {
                let mut members: Vec<Proc> = processes.iter().filter(|(pid, ..)| pids.contains(pid)).cloned().collect();
                members.sort_by_key(|(pid, ..)| (*pid != head, *pid));
                let (name, desktop_id) = self.program_name(&members);
                let service = desktop_service(&members[0].1);
                let system = service.is_some() || members.iter().any(|(_, comm, ..)| comm == "wayfire");
                let name = service.map_or(name, str::to_owned);
                let ticks: u64 = members.iter().map(|(_, _, ticks, ..)| ticks).sum();
                let counters = AppCounters {
                    usage_usec: (ticks as f64 * 1e6 / TICKS_PER_SECOND) as u64,
                    disk: members.iter().map(|(pid, ..)| io(*pid)).fold(Rate::default(), |sum, rate| Rate { read: sum.read + rate.read, write: sum.write + rate.write }),
                };
                let memory = members.iter().map(|(_, _, _, rss, _)| rss).sum();
                Cgroup { id: format!("{id}#{head}"), name, desktop_id, system, counters, memory, processes: members }
            })
            .collect()
    }

    /// A readable name for a program: the first of its processes with a desktop file. An AppImage
    /// is started by a runtime named after its version, and the app it runs is named after the app.
    fn program_name(&mut self, members: &[Proc]) -> (String, Option<String>) {
        for (_, comm, ..) in members {
            let found = self
                .commands
                .entry(comm.clone())
                .or_insert_with(|| Some((autark_common::desktop::app_name(comm)?, autark_common::desktop::desktop_id(comm)?)))
                .clone();
            if let Some((name, desktop_id)) = found {
                return (name, Some(desktop_id));
            }
        }
        (command_name(members[0].0, &members[0].1), None)
    }
}

struct Cgroup {
    id: String,
    name: String,
    desktop_id: Option<String>,
    system: bool,
    counters: AppCounters,
    memory: u64,
    processes: Vec<Proc>,
}

/// pid, name, CPU ticks so far, resident memory, parent pid.
type Proc = (u32, String, u64, u64, u32);

const RAPL: &str = "/sys/class/powercap/intel-rapl:0/energy_uj";

/// Whether we can read the CPU's energy counter ourselves, without the root helper.
pub fn rapl_readable() -> bool {
    std::fs::File::open(RAPL).is_ok()
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn collect_leaves(dir: &Path, leaves: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let children: Vec<PathBuf> = entries.flatten().filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir())).map(|entry| entry.path()).collect();
    // A cgroup with processes of its own counts even if it has children, as systemd's
    // `user@.service` does for its manager.
    if children.is_empty() || read(dir.join("cgroup.procs")).is_some_and(|procs| !procs.trim().is_empty()) {
        leaves.push(dir.to_owned());
    }
    for child in children {
        collect_leaves(&child, leaves);
    }
}

fn process(pid: u32) -> Option<Proc> {
    let stat = read(format!("/proc/{pid}/stat"))?;
    let name = stat[stat.find('(')? + 1..stat.rfind(')')?].to_owned();
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let parent = fields.get(1)?.parse::<u32>().ok()?;
    let ticks = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    let rss = fields.get(21)?.parse::<u64>().ok()? * PAGE_SIZE;
    Some((pid, name, ticks, rss, parent))
}

/// What a process has read from and written to disk so far.
fn io(pid: u32) -> Rate {
    let text = read(format!("/proc/{pid}/io")).unwrap_or_default();
    let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok()).unwrap_or(0);
    Rate { read: field("read_bytes:"), write: field("write_bytes:") }
}

/// Groups processes by the program that started them: each one whose parent isn't among them
/// (init started it, or its launcher has gone) heads a group with everything below it.
/// Takes (pid, parent pid) pairs and gives each group's head and members.
fn split_trees(processes: &[(u32, u32)]) -> Vec<(u32, Vec<u32>)> {
    let parents: HashMap<u32, u32> = processes.iter().copied().collect();
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(pid, _) in processes {
        let mut head = pid;
        // A pid reused while we read could make a loop; the limit ends it.
        for _ in 0..parents.len() {
            match parents.get(&head) {
                Some(&parent) if parent != head && parents.contains_key(&parent) => head = parent,
                _ => break,
            }
        }
        groups.entry(head).or_default().push(pid);
    }
    groups.into_iter().collect()
}

/// The parts of the desktop that run in the login session, by process name (cut at 15 characters),
/// with what to call them. Killing one would leave the screen without its bar, dock or lock,
/// so force quit leaves them out.
fn desktop_service(comm: &str) -> Option<&'static str> {
    [
        ("login", "Login session"),
        ("wayfire", "Desktop"),
        ("autark-bar", "Top bar"),
        ("autark-displays", "Displays"),
        ("autark-dock", "Dock"),
        ("autark-hw", "Hardware"),
        ("autark-launcher", "Launcher"),
        ("autark-lock", "Lock screen"),
        ("autark-nightlight", "Night light"),
        ("autark-notifyd", "Notifications"),
        ("autark-osd", "On-screen display"),
        ("autark-power", "Power"),
        ("autark-wallpaper", "Wallpaper"),
    ]
    .into_iter()
    .find(|(name, _)| name.chars().take(15).eq(comm.chars()))
    .map(|(_, title)| title)
}

/// What to call a process: its name, from the command line when `comm` was cut short, and without
/// a title such as the `: server` in `tmux: server`.
fn command_name(pid: u32, comm: &str) -> String {
    let comm = comm.split(':').next().unwrap_or(comm).trim();
    if comm.chars().count() < 15 {
        return comm.to_owned();
    }
    read(format!("/proc/{pid}/cmdline"))
        .and_then(|text| Some(Path::new(text.split('\0').next()?).file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| comm.to_owned())
}

/// A job's name for a unit that has none of its own: its command and subcommand, as in `cargo test`.
fn job_name(pid: u32, comm: &str) -> String {
    let name = command_name(pid, comm);
    let subcommand = read(format!("/proc/{pid}/cmdline")).and_then(|text| {
        let word = text.split('\0').nth(1)?;
        (!word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !word.starts_with('-')).then(|| word.to_owned())
    });
    match subcommand {
        Some(word) => format!("{name} {word}"),
        None => name,
    }
}

/// `run-p674502-i673944`: a unit made by `systemd-run` for a one-off command.
fn is_run_unit(id: &str) -> bool {
    id.strip_prefix("run-p").and_then(|rest| rest.split_once("-i")).is_some_and(|(pid, invocation)| {
        !pid.is_empty() && !invocation.is_empty() && pid.chars().chain(invocation.chars()).all(|c| c.is_ascii_digit())
    })
}

/// Marks a name made from a random unit name, to be replaced by its first process's name.
const GENERIC: &str = "\u{0}";

/// A readable name for a cgroup: `app-gnome-google\x2dchrome-2219790.scope` is Google Chrome,
/// from its desktop file when there is one.
fn name_for(leaf: &str) -> (String, Option<String>) {
    let unit = unescape(leaf);
    if unit.starts_with("session-") && unit.ends_with(".scope") {
        return ("Login session".to_owned(), None);
    }
    let mut id = unit.trim_end_matches(".scope").trim_end_matches(".service").trim_end_matches(".slice");
    id = id.split('@').next().unwrap_or(id);
    if let Some(app) = id.strip_prefix("app-") {
        id = app;
        for launcher in ["gnome-", "flatpak-", "autark-", "kde-"] {
            if let Some(rest) = id.strip_prefix(launcher) {
                id = rest;
                break;
            }
        }
        if is_uuid_suffixed(id) {
            return (format!("{id}{GENERIC}"), None);
        }
        // A trailing pid.
        if let Some((rest, suffix)) = id.rsplit_once('-')
            && suffix.chars().all(|c| c.is_ascii_digit())
            && !rest.is_empty()
        {
            id = rest;
        }
    }
    if is_uuid_suffixed(id) || is_run_unit(id) {
        return (format!("{id}{GENERIC}"), None);
    }
    if id == "init" {
        return ("System".to_owned(), None);
    }
    let desktop_id = autark_common::desktop::desktop_id(id);
    let name = autark_common::desktop::app_name(id).unwrap_or_else(|| id.to_owned());
    (name, desktop_id)
}

/// `ptyxis-spawn-d589f33c-b1a3-4a7e-9f1e-0c3f5b2a1d44`: ends in a UUID.
fn is_uuid_suffixed(id: &str) -> bool {
    let parts: Vec<&str> = id.rsplitn(6, '-').collect();
    parts.len() == 6 && parts[..5].iter().rev().map(|part| part.len()).eq([8, 4, 4, 4, 12]) && parts[..5].iter().all(|part| part.chars().all(|c| c.is_ascii_hexdigit()))
}

/// systemd escapes unit names: `\x2d` is `-`.
fn unescape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find("\\x") {
        out.push_str(&rest[..start]);
        match rest.get(start + 2..start + 4).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            Some(byte) => {
                out.push(byte as char);
                rest = &rest[start + 4..];
            }
            None => {
                out.push_str("\\x");
                rest = &rest[start + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `cpu` lines of `/proc/stat`: the whole system first, then each core.
fn parse_stat(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter(|line| line.starts_with("cpu"))
        .map(|line| {
            let fields: Vec<u64> = line.split_whitespace().skip(1).filter_map(|field| field.parse().ok()).collect();
            // user nice system idle iowait irq softirq steal; guest time is already in user.
            let total: u64 = fields.iter().take(8).sum();
            let idle = fields.get(3).copied().unwrap_or(0) + fields.get(4).copied().unwrap_or(0);
            CpuTimes { busy: total.saturating_sub(idle), total }
        })
        .collect()
}

fn parse_meminfo(text: &str) -> Memory {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let (total, available) = (field("MemTotal"), field("MemAvailable"));
    let (swap_total, swap_free) = (field("SwapTotal"), field("SwapFree"));
    Memory { total, used: total.saturating_sub(available), available, swap_total, swap_used: swap_total.saturating_sub(swap_free) }
}

/// Whole disks only, so partitions aren't counted twice.
fn is_disk(name: &str) -> bool {
    let numbered = |prefix: &str| name.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty() && !rest.contains('p'));
    let lettered = |prefix: &str| name.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_lowercase()));
    numbered("nvme") || numbered("mmcblk") || lettered("sd") || lettered("vd") || lettered("xvd")
}

fn parse_diskstats(text: &str) -> Rate {
    let mut total = Rate::default();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || !is_disk(fields[2]) {
            continue;
        }
        total.read += fields[5].parse::<u64>().unwrap_or(0) * 512;
        total.write += fields[9].parse::<u64>().unwrap_or(0) * 512;
    }
    total
}

/// Bytes received and sent on the interfaces `physical` accepts.
fn parse_net_dev(text: &str, physical: impl Fn(&str) -> bool) -> Rate {
    let mut total = Rate::default();
    for line in text.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else { continue };
        let name = name.trim();
        if name == "lo" || !physical(name) {
            continue;
        }
        let fields: Vec<u64> = counters.split_whitespace().filter_map(|field| field.parse().ok()).collect();
        total.read += fields.first().copied().unwrap_or(0);
        total.write += fields.get(8).copied().unwrap_or(0);
    }
    total
}

fn parse_cpu_stat(text: &str) -> Option<u64> {
    text.lines().find_map(|line| line.strip_prefix("usage_usec ")?.trim().parse().ok())
}

fn parse_io_stat(text: &str) -> Rate {
    let mut total = Rate::default();
    for field in text.split_whitespace() {
        if let Some(bytes) = field.strip_prefix("rbytes=") {
            total.read += bytes.parse::<u64>().unwrap_or(0);
        } else if let Some(bytes) = field.strip_prefix("wbytes=") {
            total.write += bytes.parse::<u64>().unwrap_or(0);
        }
    }
    total
}

fn battery() -> Option<Battery> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let dir = entries.flatten().map(|entry| entry.path()).find(|dir| {
        read(dir.join("type")).is_some_and(|kind| kind.trim() == "Battery") && read(dir.join("scope")).is_none_or(|scope| scope.trim() != "Device")
    })?;
    let number = |name: &str| read(dir.join(name)).and_then(|text| text.trim().parse::<f64>().ok());
    let full = number("energy_full").or_else(|| number("charge_full"));
    let design = number("energy_full_design").or_else(|| number("charge_full_design"));
    let watts = number("power_now").map(|micro| micro / 1e6).or_else(|| Some(number("current_now")? * number("voltage_now")? / 1e12)).filter(|&watts| watts > 0.0);
    Some(Battery {
        percent: number("capacity")?,
        status: read(dir.join("status")).map(|text| text.trim().to_owned()).unwrap_or_default(),
        health: full.zip(design).filter(|&(_, design)| design > 0.0).map(|(full, design)| (full / design * 100.0).min(100.0)),
        cycles: number("cycle_count").map(|cycles| cycles as u64).filter(|&cycles| cycles > 0),
        watts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat() {
        let times = parse_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\nintr 1 2");
        assert_eq!(times.len(), 2);
        assert_eq!((times[0].busy, times[0].total), (150, 1000));
    }

    #[test]
    fn meminfo() {
        let memory = parse_meminfo("MemTotal:       16000 kB\nMemFree: 1000 kB\nMemAvailable:    6000 kB\nSwapTotal: 2000 kB\nSwapFree: 1500 kB\n");
        assert_eq!(memory.used, 10000 * 1024);
        assert_eq!(memory.swap_used, 500 * 1024);
    }

    #[test]
    fn disks_skip_partitions() {
        assert!(is_disk("nvme0n1") && is_disk("sda") && is_disk("mmcblk0"));
        assert!(!is_disk("nvme0n1p2") && !is_disk("sda1") && !is_disk("loop0") && !is_disk("dm-0") && !is_disk("mmcblk0p1"));
        let rate = parse_diskstats(" 259 0 nvme0n1 10 0 100 0 5 0 40 0 0 0 0\n 259 1 nvme0n1p1 10 0 100 0 5 0 40 0 0 0 0");
        assert_eq!(rate, Rate { read: 51200, write: 20480 });
    }

    #[test]
    fn net_dev() {
        let text = "Inter-|   Receive\n face |bytes\n    lo: 500 1 0 0 0 0 0 0 500 1 0 0 0 0 0 0\n wlan0: 1000 10 0 0 0 0 0 0 300 3 0 0 0 0 0 0\ntailscale0: 70 1 0 0 0 0 0 0 70 1 0 0 0 0 0 0";
        assert_eq!(parse_net_dev(text, |name| name == "wlan0"), Rate { read: 1000, write: 300 });
    }

    #[test]
    fn cgroup_files() {
        assert_eq!(parse_cpu_stat("usage_usec 123456\nuser_usec 1\n"), Some(123456));
        assert_eq!(parse_io_stat("259:0 rbytes=100 wbytes=50 rios=1\n8:0 rbytes=1 wbytes=2"), Rate { read: 101, write: 52 });
    }

    #[test]
    fn session_programs_split_by_who_started_them() {
        // Galley's runtime (10) started the app (11), which started a helper (12); a terminal (20) runs a shell (21) running cargo (22).
        // Process 30's launcher (99) has gone, so it heads a group of its own.
        let groups = split_trees(&[(10, 1), (11, 10), (12, 11), (20, 1), (21, 20), (22, 21), (30, 99)]);
        assert_eq!(groups, vec![(10, vec![10, 11, 12]), (20, vec![20, 21, 22]), (30, vec![30])]);
    }

    #[test]
    fn session_programs_survive_a_parent_loop() {
        // Pids can be reused while we read; two processes naming each other must still end up somewhere.
        let groups = split_trees(&[(5, 6), (6, 5), (7, 7)]);
        assert_eq!(groups.iter().map(|(_, pids)| pids.len()).sum::<usize>(), 3);
    }

    #[test]
    fn desktop_services_by_cut_name() {
        assert_eq!(desktop_service("wayfire"), Some("Desktop"));
        assert_eq!(desktop_service("autark-bar"), Some("Top bar"));
        assert_eq!(desktop_service("autark-nightlig"), Some("Night light"));
        assert_eq!(desktop_service("autark-wallpape"), Some("Wallpaper"));
        assert!(desktop_service("galley").is_none() && desktop_service("autark-terminal").is_none() && desktop_service("autark-files").is_none());
    }

    #[test]
    fn one_off_jobs_are_named_for_their_command() {
        assert!(is_run_unit("run-p674502-i673944") && !is_run_unit("run-parser") && !is_run_unit("run-p12-i") && !is_run_unit("run-pxx-i1"));
        assert!(name_for("run-p674502-i673944.scope").0.ends_with(GENERIC));
        assert_eq!(name_for("init.scope").0, "System");
        // This test's own process: a short name stays as it is, and a title after a colon is dropped.
        assert_eq!(command_name(std::process::id(), "tmux: server"), "tmux");
    }

    #[test]
    fn unit_names() {
        assert_eq!(unescape("app-gnome-google\\x2dchrome-2219790.scope"), "app-gnome-google-chrome-2219790.scope");
        assert_eq!(name_for("session-3.scope").0, "Login session");
        assert_eq!(name_for("app-gnome-not\\x2dinstalled\\x2dapp-42.scope").0, "not-installed-app");
        assert_eq!(name_for("pipewire.service").0, "pipewire");
        assert!(name_for("app-ptyxis-spawn-d589f33c-b1a3-4a7e-9f1e-0c3f5b2a1d44.scope").0.ends_with(GENERIC));
    }
}
