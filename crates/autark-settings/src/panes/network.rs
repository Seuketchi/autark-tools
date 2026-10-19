//! Network: what systemd-networkd and systemd-resolved say about each link and the DNS in use, read with
//! `networkctl` and `resolvectl`. Wi-Fi is joined on its own pane. Changing addresses or DNS means writing root's files in
//! `/etc/systemd`, so this pane shows them and doesn't pretend to edit them.

use std::collections::BTreeMap;
use std::process::Command;

use gtk::prelude::*;
use serde_json::Value;

use crate::{privileged, ui};

const PRESETS: [(&str, &str, &[&str]); 4] = [
    ("quad9", "Quad9", &["9.9.9.9#dns.quad9.net", "149.112.112.112#dns.quad9.net", "2620:fe::fe#dns.quad9.net", "2620:fe::9#dns.quad9.net"]),
    ("cloudflare", "Cloudflare", &["1.1.1.1#cloudflare-dns.com", "1.0.0.1#cloudflare-dns.com"]),
    ("google", "Google", &["8.8.8.8#dns.google", "8.8.4.4#dns.google"]),
    ("adguard", "AdGuard (blocks ads)", &["94.140.14.14#dns.adguard-dns.com", "94.140.15.15#dns.adguard-dns.com"]),
];
const TLS: [(&str, &str); 3] = [("yes", "Required"), ("opportunistic", "When the server can"), ("no", "Off")];

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Network", Some("What the network is doing, and which DNS servers it asks. Join Wi-Fi networks on the Wi-Fi pane.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fill(&body, &status);
    column.append(&body);
    column.append(&status);
    let refresh = ui::button("Refresh", {
        let (body, status) = (body.clone(), status.clone());
        move |_| fill(&body, &status)
    });
    column.append(&refresh);
    page.upcast()
}

fn fill(body: &gtk::Box, status: &gtk::Label) {
    ui::clear(body);
    let Some(list) = json(&["networkctl", "--json=short", "list"]) else {
        body.append(&ui::note("networkctl isn't answering, so there's nothing to show."));
        return;
    };
    let links: Vec<&Value> = list["Interfaces"].as_array().map(|all| all.iter().filter(|link| link["Name"] != "lo").collect()).unwrap_or_default();
    if links.is_empty() {
        body.append(&ui::note("No network devices found."));
    }
    for link in links {
        let name = link["Name"].as_str().unwrap_or("");
        let detail = json(&["networkctl", "--json=short", "status", name]).unwrap_or(Value::Null);
        body.append(&ui::section(&heading(link), None, &link_rows(link, &detail)));
    }
    body.append(&ui::section("DNS", None, &dns_rows(body, status)));
}

fn heading(link: &Value) -> String {
    let kind = match link["Type"].as_str() {
        Some("ether") => "Wired",
        Some("wlan") => "Wi-Fi",
        Some(other) => other,
        None => "Link",
    };
    format!("{kind} · {}", link["Name"].as_str().unwrap_or(""))
}

fn link_rows(link: &Value, detail: &Value) -> gtk::Box {
    let group = ui::group();
    let state = match link["OperationalState"].as_str().unwrap_or("") {
        "routable" | "enslaved" => "Connected",
        "degraded" => "Connected, limited",
        "no-carrier" => "Cable unplugged",
        "off" | "dormant" => "Not connected",
        other if !other.is_empty() => other,
        _ => "Unknown",
    };
    let state = match detail["SSID"].as_str() {
        Some(ssid) if state.starts_with("Connected") => format!("{state} to {ssid}"),
        _ => state.to_owned(),
    };
    let end = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    end.append(&value(&state));
    if link["Type"] == "wlan" {
        let open = ui::button("Wi-Fi settings…", |button| {
            let _ = button.activate_action("win.show-pane", Some(&"wifi".to_variant()));
        });
        end.append(&open);
    }
    group.append(&ui::row("Status", None, &end));
    if let Some(address) = first_ipv4(detail) {
        group.append(&ui::row("IP address", None, &value(&address.0)));
        if let Some(router) = address.1 {
            group.append(&ui::row("Router", None, &value(&router)));
        }
    }
    let dns: Vec<&str> = detail["DNS"].as_array().map(|servers| servers.iter().filter_map(|server| server["AddressString"].as_str()).collect()).unwrap_or_default();
    if !dns.is_empty() {
        group.append(&ui::row("DNS from this link", None, &value(&dns.join(", "))));
    }
    if let Some(mac) = detail["HardwareAddress"].as_array() {
        let mac: Vec<String> = mac.iter().filter_map(Value::as_u64).map(|byte| format!("{byte:02x}")).collect();
        if !mac.is_empty() {
            group.append(&ui::row("Hardware address", None, &value(&mac.join(":"))));
        }
    }
    let model = [detail["Vendor"].as_str(), detail["Model"].as_str()].iter().flatten().copied().collect::<Vec<_>>().join(" ");
    if !model.is_empty() {
        group.append(&ui::row("Device", Some(detail["Driver"].as_str().unwrap_or("")), &value(&model)));
    }
    group
}

/// The first IPv4 address with its prefix, and who handed it out.
fn first_ipv4(detail: &Value) -> Option<(String, Option<String>)> {
    let addresses = detail["Addresses"].as_array()?;
    let address = addresses.iter().find(|address| address["Family"] == 2)?;
    let text = format!("{}/{}", address["AddressString"].as_str()?, address["PrefixLength"].as_u64().unwrap_or(0));
    Some((text, address["ConfigProviderString"].as_str().map(str::to_owned)))
}

/// The DNS settings in effect: every file in `resolved.conf.d` in order, later files winning, as systemd reads them.
pub fn resolved_config() -> BTreeMap<String, String> {
    let mut files: Vec<std::path::PathBuf> = vec!["/etc/systemd/resolved.conf".into()];
    let mut dropins: Vec<std::path::PathBuf> = std::fs::read_dir("/etc/systemd/resolved.conf.d").into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "conf")).collect();
    dropins.sort();
    files.extend(dropins);
    let mut values = BTreeMap::new();
    for file in files {
        for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
            if let Some((key, value)) = line.split_once('=') {
                if !line.trim_start().starts_with(['#', ';']) {
                    values.insert(key.trim().to_owned(), value.trim().to_owned());
                }
            }
        }
    }
    values
}

/// What the settings drop-in says about DNS: "default" (it says nothing), "network", a preset's id, or "custom".
pub(super) fn dns_mode() -> (String, String) {
    let ours = std::fs::read_to_string("/etc/systemd/resolved.conf.d/zz-autark-settings.conf").unwrap_or_default();
    let ours: BTreeMap<String, String> = ours.lines().filter_map(|line| line.split_once('=')).map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned())).collect();
    match ours.get("DNS") {
        None => ("default".to_owned(), String::new()),
        Some(servers) if servers.is_empty() => ("network".to_owned(), String::new()),
        Some(servers) => {
            let preset = PRESETS.iter().find(|(_, _, list)| list.join(" ") == *servers).map(|(id, _, _)| (*id).to_owned());
            (preset.unwrap_or_else(|| "custom".to_owned()), servers.clone())
        }
    }
}

fn dns_rows(body: &gtk::Box, status: &gtk::Label) -> gtk::Box {
    let group = ui::group();
    let (mode, servers) = dns_mode();
    let config = resolved_config();
    let tls = config.get("DNSOverTLS").map(String::as_str).unwrap_or("no").to_owned();

    let mut options: Vec<(&str, &str)> = vec![("default", "Autark's choice (Quad9)"), ("network", "From the network")];
    options.extend(PRESETS.iter().map(|(id, name, _)| (*id, *name)));
    options.push(("custom", "My own servers"));
    let servers_dropdown = ui::choice(&options, &mode, {
        let (body, status, tls) = (body.clone(), status.clone(), tls.clone());
        move |choice| {
            let preset = PRESETS.iter().find(|(id, _, _)| *id == choice);
            let mut args: Vec<&str> = vec!["resolved-dns"];
            match (choice, preset) {
                ("default", _) => args.push("default"),
                ("network", _) => args.push("network"),
                (_, Some((_, _, list))) => {
                    args.push("custom");
                    args.extend(list.iter());
                }
                _ => {
                    // "My own servers" waits for addresses typed in the box below.
                    return;
                }
            }
            if choice == "network" && tls == "yes" {
                status.set_label("Required encryption needs servers with a name, which the network's don't have. Choose “When the server can” first.");
                status.set_visible(true);
                return;
            }
            let (body, status_for_after) = (body.clone(), status.clone());
            privileged::apply(&body.clone(), "change the DNS servers", &args, None, &status, move |_| fill(&body, &status_for_after));
        }
    });
    group.append(&ui::row("DNS servers", Some("Who answers when this laptop looks up a name"), &servers_dropdown));

    let own = ui::field(&servers, 38, {
        let (body, status, tls) = (body.clone(), status.clone(), tls.clone());
        move |text| {
            let words: Vec<String> = text.split([' ', ',']).filter(|word| !word.is_empty()).map(str::to_owned).collect();
            let fine = !words.is_empty() && words.len() <= 6 && words.iter().all(|word| word.split('#').next().is_some_and(|address| address.parse::<std::net::IpAddr>().is_ok()));
            if !fine {
                return false;
            }
            if tls == "yes" && words.iter().any(|word| !word.contains('#')) {
                status.set_label("Required encryption needs each server written as address#name, like 9.9.9.9#dns.quad9.net.");
                status.set_visible(true);
                return false;
            }
            let mut args: Vec<&str> = vec!["resolved-dns", "custom"];
            args.extend(words.iter().map(String::as_str));
            let (body, status_for_after) = (body.clone(), status.clone());
            privileged::apply(&body.clone(), "change the DNS servers", &args, None, &status, move |_| fill(&body, &status_for_after));
            true
        }
    });
    own.set_placeholder_text(Some("9.9.9.9 149.112.112.112"));
    own.set_sensitive(mode == "custom" || mode == "default");
    group.append(&ui::row("Your own servers", Some("Addresses with spaces between; add #name for encryption. Press Enter to apply."), &own));

    let tls_dropdown = ui::choice(&TLS, &tls, {
        let (body, status, mode) = (body.clone(), status.clone(), mode.clone());
        move |choice| {
            if choice == "yes" && mode == "network" {
                status.set_label("Required encryption needs servers with a name; the network's DNS doesn't have one. Pick Quad9, Cloudflare or your own first.");
                status.set_visible(true);
                return;
            }
            let (body, status_for_after) = (body.clone(), status.clone());
            privileged::apply(&body.clone(), "change whether DNS is encrypted", &["resolved-tls", choice], None, &status, move |_| fill(&body, &status_for_after));
        }
    });
    group.append(&ui::row("Encrypted DNS", Some("DNS over TLS: lookups can't be read or changed on the way"), &tls_dropdown));

    let servers: Vec<String> = json(&["resolvectl", "--json=short", "status"])
        .as_ref()
        .and_then(|all| all.as_array())
        .and_then(|all| all.first())
        .and_then(|global| global["servers"].as_array())
        .map(|all| all.iter().filter_map(|server| server["addressString"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if !servers.is_empty() {
        group.append(&ui::row("In use now", None, &value(&servers.join("\n"))));
    }
    group
}

fn value(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("value");
    label.set_xalign(1.0);
    label.set_selectable(true);
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Right);
    label
}

fn text(command: &[&str]) -> String {
    Command::new(command[0]).args(&command[1..]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
}

fn json(command: &[&str]) -> Option<Value> {
    serde_json::from_str(&text(command)).ok()
}
