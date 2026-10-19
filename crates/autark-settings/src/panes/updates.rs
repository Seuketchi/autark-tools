//! Software Updates: what's installed and what the security watch found. Applying updates is `autark-pkg upgrade`,
//! which needs root and the recipes, so this pane tells you the state and the command and doesn't run it. The security
//! report is `autark-cve-watch`'s, which compares the installed versions with the NVD's advisories.

use std::process::Command;

use gtk::prelude::*;

use crate::ui;

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Software Updates", None));

    let system = ui::group();
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| release.lines().find_map(|line| line.strip_prefix(&format!("{key}=")).map(|value| value.trim_matches('"').to_owned())).unwrap_or_default();
    system.append(&ui::row("Autark", None, &ui::value(&format!("{} {}", field("PRETTY_NAME"), field("VERSION_ID")).trim().to_owned())));
    let kernel = Command::new("uname").arg("-r").output().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()).unwrap_or_default();
    system.append(&ui::row("Kernel", None, &ui::value(&kernel)));
    column.append(&ui::section("This system", None, &system));

    let report = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state/autark-cve-watch/report.txt");
    let watch = ui::group();
    match std::fs::read_to_string(&report) {
        Ok(text) => {
            let mut lines = text.lines();
            let header = lines.next().unwrap_or("").replace("autark-cve-watch ", "");
            watch.append(&ui::row("Last checked", Some("By the weekly security watch"), &ui::value(&header)));
            for line in lines.filter(|line| !line.trim().is_empty()) {
                if let Some((name, version, summary)) = row(line) {
                    watch.append(&ui::row(&format!("{name} {version}"), None, &ui::value(&summary)));
                }
            }
        }
        Err(_) => watch.append(&ui::row("Security watch", Some("It hasn't run yet"), &ui::value("No report"))),
    }
    column.append(&ui::section("Security watch", None, &watch));
    column.append(&ui::note(
        "Many of the advisories listed are for versions older than the one installed, so a count isn't a sign something is wrong. To install newer versions, run autark-pkg upgrade in a root shell.",
    ));
    page.upcast()
}

/// "openssl     4.0.1       24 CVEs, 14 high/critical   CVE-… [why it matters]" as name, version and a short summary.
fn row(line: &str) -> Option<(String, String, String)> {
    let mut words = line.split_whitespace();
    let (name, version) = (words.next()?.to_owned(), words.next()?.to_owned());
    let rest: Vec<&str> = words.collect();
    let rest = rest.join(" ");
    let counts = rest.split("   ").next().unwrap_or(&rest);
    let counts = counts.split(" CVE-").next().unwrap_or(counts).trim();
    Some((name, version, counts.to_owned()))
}
