//! `autark-hw`: what's in the machine and what runs it.
//!
//!   autark-hw devices   every PCI and USB device, with its driver
//!   autark-hw doctor    only what needs attention, and what to do about it
//!   autark-hw controls  the laptop's backlights, battery, fans, radios and power profile
//!   autark-hw watch     stays running, and says when hardware that needs attention appears

mod gui;
mod watch;

use autark_hw::aliases;
use autark_hw::controls;
use autark_hw::devices::{self, Class};
use autark_hw::doctor::{self, Finding, Machine, Status};
use autark_hw::kernel_log;

const USAGE: &str = "usage: autark-hw [devices|doctor|controls|watch]   (no command opens the window)";

fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|text| text.trim().to_owned()).unwrap_or_default()
}

fn findings() -> (Vec<Finding>, Vec<kernel_log::Note>) {
    let devices = devices::read();
    let aliases = aliases::load(&kernel());
    let loaded = doctor::loaded_modules();
    let notes = kernel_log::read();
    let findings = doctor::diagnose(&Machine { devices: &devices, aliases: &aliases, loaded: &loaded, notes: &notes, firmware_root: std::path::Path::new("/lib/firmware") });
    (findings, notes)
}

fn mark(finding: &Finding) -> &'static str {
    match (&finding.status, finding.is_problem()) {
        (Status::Quiet, _) => "·",
        (_, false) => "✓",
        (Status::NoDriver, _) => "✗",
        _ => "!",
    }
}

fn main() -> std::process::ExitCode {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        None => return gui::run(),
        Some("watch") => return watch::run(),
        Some("devices") => {
            let (findings, _) = findings();
            let mut shown: Option<Class> = None;
            for finding in &findings {
                if shown != Some(finding.device.class) {
                    shown = Some(finding.device.class);
                    println!("\n{}", finding.device.class.label());
                }
                let drivers = if finding.device.drivers.is_empty() { "no driver".to_owned() } else { finding.device.drivers.join(", ") };
                println!("  {} {:<9} {:<46} {drivers}", mark(finding), finding.device.id, finding.device.title());
            }
        }
        Some("doctor") => {
            let (findings, notes) = findings();
            let problems: Vec<&Finding> = findings.iter().filter(|finding| finding.is_problem()).collect();
            for finding in &problems {
                println!("{} {} ({})", mark(finding), finding.device.title(), finding.device.id);
                if let Some(advice) = doctor::advice(finding) {
                    println!("    {advice}");
                }
                for text in &finding.unsupported {
                    println!("    kernel: {text}");
                }
            }
            // Complaints from drivers that run no PCI or USB device: a laptop's embedded controller, say.
            for note in doctor::loose_notes(&findings, &notes) {
                println!("! {}\n    kernel: {}", note.driver, note.text);
            }
            if problems.is_empty() && doctor::loose_notes(&findings, &notes).is_empty() {
                println!("Every device that needs a driver has one.");
            }
        }
        Some("controls") => {
            let controls = controls::detect(std::path::Path::new("/sys"));
            println!("{} {}", controls.vendor, controls.product);
            let slider = |slider: &controls::Slider| println!("  {:<22} {} / {}   (via {} {})", slider.label, slider.value, slider.max, slider.subsystem, slider.name);
            controls.brightness.iter().for_each(slider);
            if controls.lights.is_empty() {
                println!("  Keyboard backlight     not offered by the kernel");
            }
            controls.lights.iter().for_each(slider);
            for battery in &controls.batteries {
                let health = battery.health.map_or(String::new(), |health| format!(", {health}% of its original capacity"));
                let cycles = battery.cycles.map_or(String::new(), |cycles| format!(", {cycles} cycles"));
                println!("  Battery {:<14} {}% {}{health}{cycles}", battery.name, battery.percent, battery.status.to_lowercase());
                match &battery.limit {
                    Some(limit) => println!("    charge limit {}% ({})", limit.percent, if limit.writable { "you can change it" } else { "only root can change it" }),
                    None => println!("    no charge limit offered by the kernel"),
                }
            }
            for reading in controls.fans.iter().chain(&controls.temperatures) {
                println!("  {:<22} {}", reading.label, reading.value);
            }
            for radio in &controls.radios {
                let state = if radio.hard_blocked { "off by a switch" } else if radio.soft_blocked { "off" } else { "on" };
                println!("  {:<22} {state}", format!("{} ({})", radio.kind, radio.name));
            }
            match &controls.profile {
                Some(profile) => println!("  Power profile          {} of {}", profile.current, profile.choices.join(", ")),
                None => println!("  Power profile          not offered by the kernel"),
            }
            for mode in &controls.modes {
                println!("  {:<22} {} of {}", mode.label, mode.current, mode.choices.join(", "));
            }
        }
        _ => eprintln!("{USAGE}"),
    }
    std::process::ExitCode::SUCCESS
}
