//! Root helper for autark-stats. The kernel keeps the CPU's RAPL energy counter root-only, since
//! reading it thousands of times a second leaks secrets (the PLATYPUS attack). This helper reads
//! it for everyone, but only answers with the average power over at least a second, far too
//! coarse for that. D-Bus starts it on demand (`org.autark.StatsPower1` on the system bus) and
//! it exits after a minute without calls.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const COUNTER: &str = "/sys/class/powercap/intel-rapl:0/energy_uj";
const RANGE: &str = "/sys/class/powercap/intel-rapl:0/max_energy_range_uj";
/// The shortest stretch an answer averages over.
const WINDOW: Duration = Duration::from_secs(1);
const IDLE: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Reading {
    /// The last counter value and when it was read.
    last: Option<(Instant, u64)>,
    watts: Option<f64>,
}

struct Power {
    reading: Mutex<Reading>,
    called: Mutex<Instant>,
}

fn counter() -> zbus::fdo::Result<u64> {
    std::fs::read_to_string(COUNTER)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| zbus::fdo::Error::NotSupported(format!("can't read {COUNTER}")))
}

/// Energy used between two counter values, allowing for the counter wrapping around.
fn used(then: u64, now: u64) -> u64 {
    if now >= then {
        now - then
    } else {
        let range = std::fs::read_to_string(RANGE).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(u64::MAX);
        range - then + now
    }
}

#[zbus::interface(name = "org.autark.StatsPower1")]
impl Power {
    /// The CPU package's average power in watts since the previous answer, over at least a
    /// second. Calls within a second of each other get the same answer.
    async fn watts(&self) -> zbus::fdo::Result<f64> {
        *self.called.lock().unwrap() = Instant::now();
        let last = self.reading.lock().unwrap().last;
        let (start, start_energy) = match last {
            Some((at, _)) if at.elapsed() < WINDOW => {
                if let Some(watts) = self.reading.lock().unwrap().watts {
                    return Ok(watts);
                }
                last.unwrap()
            }
            // A reading long ago would average over a stretch nobody asked about.
            Some(last) if last.0.elapsed() < Duration::from_secs(30) => last,
            _ => {
                let first = (Instant::now(), counter()?);
                self.reading.lock().unwrap().last = Some(first);
                first
            }
        };
        let wait = WINDOW.saturating_sub(start.elapsed());
        if !wait.is_zero() {
            async_io::Timer::after(wait).await;
        }
        let (now, energy) = (Instant::now(), counter()?);
        let watts = used(start_energy, energy) as f64 / 1e6 / now.duration_since(start).as_secs_f64();
        *self.reading.lock().unwrap() = Reading { last: Some((now, energy)), watts: Some(watts) };
        Ok(watts)
    }
}

fn main() {
    let result = async_io::block_on(async {
        let power = Power { reading: Mutex::default(), called: Mutex::new(Instant::now()) };
        let connection = zbus::connection::Builder::system().name("org.autark.StatsPower1").serve_at("/org/autark/StatsPower1", power).build().await?;
        let iface = connection.object_server().interface::<_, Power>("/org/autark/StatsPower1").await?;
        loop {
            async_io::Timer::after(Duration::from_secs(10)).await;
            if iface.get().await.called.lock().unwrap().elapsed() > IDLE {
                return zbus::Result::Ok(());
            }
        }
    });
    if let Err(err) = result {
        eprintln!("stats-power: {err}");
        std::process::exit(1);
    }
}
