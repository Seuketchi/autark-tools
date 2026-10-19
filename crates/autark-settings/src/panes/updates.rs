//! Software Updates: the versions, the security watch's report, and the buttons: check what `autark-pkg upgrade` would
//! change (a dry run), install it, and run the security watch now. Checking and installing are root's, so they go through
//! the helper with your password (see `privileged.rs`); installing is only offered after a check has worked, and the
//! helper runs only `autark-pkg` from its own recipes folder, never one a user can write to. The security report is
//! `autark-cve-watch`'s, which compares the installed versions with the NVD's advisories; running it needs no root.

use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::{privileged, ui};

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Software Updates", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let system = ui::group();
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| release.lines().find_map(|line| line.strip_prefix(&format!("{key}=")).map(|value| value.trim_matches('"').to_owned())).unwrap_or_default();
    system.append(&ui::row("Autark", None, &ui::value(&format!("{} {}", field("PRETTY_NAME"), field("VERSION_ID")).trim().to_owned())));
    let kernel = Command::new("uname").arg("-r").output().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()).unwrap_or_default();
    system.append(&ui::row("Kernel", None, &ui::value(&kernel)));
    column.append(&ui::section("This system", None, &system));

    // Packages: check, then install what the check found.
    let packages = ui::group();
    let result = gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).visible(false).build();
    result.add_css_class("value");
    result.add_css_class("mono");
    let checked = Rc::new(RefCell::new(false));
    let install = ui::button("Install updates", {
        let (status, result) = (status.clone(), result.clone());
        move |button| {
            button.set_sensitive(false);
            result.set_label("Installing… this can take a long time, and the window may look still.");
            result.set_visible(true);
            let (status_after, result_after, button_after) = (status.clone(), result.clone(), button.clone());
            privileged::apply(button, "install updates", &["pkg-upgrade"], None, &status, move |text| {
                result_after.set_label(if text.trim().is_empty() { "Done." } else { text.trim() });
                button_after.set_sensitive(true);
                let _ = &status_after;
            });
        }
    });
    install.set_sensitive(false);
    let check = ui::button("Check for updates", {
        let (status, result, install, checked) = (status.clone(), result.clone(), install.clone(), checked.clone());
        move |button| {
            button.set_sensitive(false);
            result.set_label("Checking…");
            result.set_visible(true);
            let (result_after, install_after, button_after, checked_after) = (result.clone(), install.clone(), button.clone(), checked.clone());
            let (status_failed, result_failed, button_failed) = (status.clone(), result.clone(), button.clone());
            // The helper's failure shows in `status`; the check button must come back either way.
            privileged::run(button.root().and_downcast::<gtk::Window>().as_ref(), "check for updates", vec!["pkg-check".to_owned()], None, move |outcome| {
                button_after.set_sensitive(true);
                match outcome {
                    Ok(text) => {
                        result_after.set_label(if text.trim().is_empty() { "Everything is up to date." } else { text.trim() });
                        *checked_after.borrow_mut() = true;
                        install_after.set_sensitive(!text.trim().is_empty());
                        status_failed.set_visible(false);
                    }
                    Err(error) => {
                        result_failed.set_visible(false);
                        button_failed.set_sensitive(true);
                        status_failed.set_label(&error);
                        status_failed.set_visible(true);
                    }
                }
            });
        }
    });
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&check);
    buttons.append(&install);
    packages.append(&ui::row("Packages", Some("Check shows what would change; nothing is installed until you press Install"), &buttons));
    column.append(&ui::section("Updates", None, &packages));
    column.append(&result);

    let watch = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fill_watch(&watch, &status);
    column.append(&watch);
    column.append(&ui::note(
        "Many of the advisories listed are for versions older than the one installed, so a count isn't a sign something is wrong. Installing needs the package recipes in /usr/src/autark/scripts/pkg.",
    ));
    column.append(&status);
    page.upcast()
}

fn fill_watch(watch: &gtk::Box, status: &gtk::Label) {
    ui::clear(watch);
    let report = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state/autark-cve-watch/report.txt");
    let group = ui::group();
    let now = ui::button("Check now", {
        let (watch, status) = (watch.clone(), status.clone());
        move |button| {
            button.set_sensitive(false);
            button.set_label("Checking…");
            let (watch, status) = (watch.clone(), status.clone());
            glib::spawn_future_local(async move {
                let outcome = gtk::gio::spawn_blocking(|| Command::new("systemctl").args(["--user", "start", "autark-cve-watch.service"]).output()).await;
                match outcome {
                    Ok(Ok(output)) if output.status.success() => status.set_visible(false),
                    Ok(Ok(output)) => {
                        status.set_label(&format!("The security watch didn't run: {}", String::from_utf8_lossy(&output.stderr).trim()));
                        status.set_visible(true);
                    }
                    _ => {
                        status.set_label("Couldn't start the security watch.");
                        status.set_visible(true);
                    }
                }
                fill_watch(&watch, &status);
            });
        }
    });
    match std::fs::read_to_string(&report) {
        Ok(text) => {
            let mut lines = text.lines();
            let header = lines.next().unwrap_or("").replace("autark-cve-watch ", "");
            group.append(&ui::row("Last checked", Some("Weekly, and when you press Check now"), &row_end(&header, &now)));
            for line in lines.filter(|line| !line.trim().is_empty()) {
                if let Some((name, version, summary)) = row(line) {
                    group.append(&ui::row(&format!("{name} {version}"), None, &ui::value(&summary)));
                }
            }
        }
        Err(_) => group.append(&ui::row("Security watch", Some("It hasn't run yet"), &now)),
    }
    watch.append(&ui::section("Security watch", None, &group));
}

fn row_end(text: &str, button: &gtk::Button) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&ui::value(text));
    line.append(button);
    line
}

/// "openssl     4.0.1       24 CVEs, 14 high/critical   CVE-… [why it matters]" as name, version and a short summary.
fn row(line: &str) -> Option<(String, String, String)> {
    let mut words = line.split_whitespace();
    let (name, version) = (words.next()?.to_owned(), words.next()?.to_owned());
    let rest: Vec<&str> = words.collect();
    let rest = rest.join(" ");
    let counts = rest.split("   ").next().unwrap_or(&rest);
    let counts = counts.split(" CVE-").next().unwrap_or(counts).trim();
    let counts = counts.split(" [").next().unwrap_or(counts).trim();
    Some((name, version, counts.replace("1 CVEs", "1 CVE")))
}
