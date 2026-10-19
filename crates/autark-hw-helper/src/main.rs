//! Root helper for autark-hw, started by D-Bus on demand as `org.autark.Hardware1` on the system bus; it exits after a
//! minute without calls. It does two things, each only for what `policy` allows and only after polkit agrees:
//!   WriteSetting  sets a power profile, a battery charge limit or charging behaviour
//!   LoadModule    loads a kernel module that drives hardware that's here
//! and answers `Loadable` for anyone: the modules that could be loaded for the hardware that's here.

mod policy;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use autark_hw::doctor::{self, Machine, Status};
use autark_hw::{aliases, devices};
use zbus::message::Header;

const IDLE: Duration = Duration::from_secs(60);

#[zbus::proxy(interface = "org.freedesktop.PolicyKit1.Authority", default_service = "org.freedesktop.PolicyKit1", default_path = "/org/freedesktop/PolicyKit1/Authority")]
trait Authority {
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, zbus::Variant<'_>>),
        action_id: &str,
        details: HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// Asks polkit whether whoever called is allowed `action`, letting it ask them (flag 1) if its rules say they may be asked.
async fn authorize(connection: &zbus::Connection, header: &Header<'_>, action: &str) -> zbus::fdo::Result<()> {
    let sender = header.sender().ok_or_else(|| zbus::fdo::Error::AuthFailed("no sender".to_owned()))?;
    let authority = AuthorityProxy::new(connection).await.map_err(|error| zbus::fdo::Error::Failed(format!("polkit: {error}")))?;
    let subject = ("system-bus-name", HashMap::from([("name", zbus::Variant::from(sender.as_str()))]));
    let (authorized, _challenge, _details) = authority.check_authorization(&subject, action, HashMap::new(), 1, "").await.map_err(|error| zbus::fdo::Error::Failed(format!("polkit: {error}")))?;
    if authorized { Ok(()) } else { Err(zbus::fdo::Error::AuthFailed(format!("not allowed: {action}"))) }
}

struct Helper {
    called: Mutex<Instant>,
}

/// The modules that drive hardware here and aren't loaded, by looking at this machine as root sees it.
fn loadable() -> Vec<String> {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|text| text.trim().to_owned()).unwrap_or_default();
    let devices = devices::read();
    let aliases = aliases::load(&kernel);
    let loaded = doctor::loaded_modules();
    let findings = doctor::diagnose(&Machine { devices: &devices, aliases: &aliases, loaded: &loaded, notes: &[], firmware_root: Path::new("/lib/firmware") });
    let mut modules: Vec<String> = findings.into_iter().filter_map(|finding| if let Status::Loadable(modules) = finding.status { Some(modules) } else { None }).flatten().collect();
    modules.sort();
    modules.dedup();
    modules
}

#[zbus::interface(name = "org.autark.Hardware1")]
impl Helper {
    /// Sets one of the settings `policy` knows, such as `/sys/firmware/acpi/platform_profile`, to a value it takes.
    async fn write_setting(&self, #[zbus(header)] header: Header<'_>, #[zbus(connection)] connection: &zbus::Connection, path: String, value: String) -> zbus::fdo::Result<()> {
        *self.called.lock().unwrap() = Instant::now();
        let (file, text) = policy::setting(Path::new("/sys"), &path, &value).map_err(zbus::fdo::Error::InvalidArgs)?;
        authorize(connection, &header, "org.autark.hardware.write-setting").await?;
        std::fs::OpenOptions::new().write(true).open(&file).and_then(|mut file| file.write_all(text.as_bytes())).map_err(|error| zbus::fdo::Error::Failed(format!("{}: {error}", file.display())))
    }

    /// Loads `name`, but only if it's one that `Loadable` lists: hardware that's here has it for a driver.
    async fn load_module(&self, #[zbus(header)] header: Header<'_>, #[zbus(connection)] connection: &zbus::Connection, name: String) -> zbus::fdo::Result<()> {
        *self.called.lock().unwrap() = Instant::now();
        if !policy::module_name(&name) {
            return Err(zbus::fdo::Error::InvalidArgs("that isn't a module name".to_owned()));
        }
        let candidates = blocking::unblock(loadable).await;
        if !candidates.iter().any(|candidate| candidate.replace('-', "_") == name.replace('-', "_")) {
            return Err(zbus::fdo::Error::AccessDenied("no hardware here needs that module".to_owned()));
        }
        authorize(connection, &header, "org.autark.hardware.load-driver").await?;
        let output = blocking::unblock(move || Command::new("/usr/sbin/modprobe").arg(&name).output()).await.map_err(|error| zbus::fdo::Error::Failed(format!("modprobe: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(zbus::fdo::Error::Failed(String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("modprobe failed").to_owned()))
        }
    }

    /// The modules that could be loaded for the hardware that's here.
    async fn loadable(&self) -> Vec<String> {
        *self.called.lock().unwrap() = Instant::now();
        blocking::unblock(loadable).await
    }
}

fn main() {
    let result = async_io::block_on(async {
        let helper = Helper { called: Mutex::new(Instant::now()) };
        let connection = zbus::connection::Builder::system().name("org.autark.Hardware1").serve_at("/org/autark/Hardware1", helper).build().await?;
        let iface = connection.object_server().interface::<_, Helper>("/org/autark/Hardware1").await?;
        loop {
            async_io::Timer::after(Duration::from_secs(10)).await;
            if iface.get().await.called.lock().unwrap().elapsed() > IDLE {
                return zbus::Result::Ok(());
            }
        }
    });
    if let Err(error) = result {
        eprintln!("hardware helper: {error}");
        std::process::exit(1);
    }
}
