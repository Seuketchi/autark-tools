//! Network: what systemd-networkd and systemd-resolved say about each link and the DNS in use, read with
//! `networkctl` and `resolvectl`. Wi-Fi is joined on its own pane. Changing addresses or DNS means writing root's files in
//! `/etc/systemd`, so this pane shows them and doesn't pretend to edit them.

use std::process::Command;

use gtk::prelude::*;
use serde_json::Value;

use crate::ui;

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Network", Some("What the system's network is doing. Join Wi-Fi networks on the Wi-Fi pane.")));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fill(&body);
    column.append(&body);
    let refresh = ui::button("Refresh", {
        let body = body.clone();
        move |_| fill(&body)
    });
    column.append(&refresh);
    page.upcast()
}

fn fill(body: &gtk::Box) {
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
    body.append(&ui::section("DNS", None, &dns_rows()));
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
    group.append(&ui::row("Status", None, &value(&state)));
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

fn dns_rows() -> gtk::Box {
    let group = ui::group();
    let status = json(&["resolvectl", "--json=short", "status"]);
    let global = status.as_ref().and_then(|all| all.as_array()).and_then(|all| all.first());
    let servers: Vec<String> = global
        .and_then(|global| global["servers"].as_array())
        .map(|all| all.iter().filter_map(|server| Some(match server["name"].as_str() {
            Some(name) => format!("{} ({name})", server["addressString"].as_str()?),
            None => server["addressString"].as_str()?.to_owned(),
        })).collect())
        .unwrap_or_default();
    if servers.is_empty() {
        group.append(&ui::row("Servers", None, &value("From the network (DHCP)")));
    } else {
        group.append(&ui::row("Servers", None, &value(&servers.join("\n"))));
    }
    let tls = text(&["resolvectl", "dnsovertls"]);
    let tls = if tls.contains("yes") || tls.contains("opportunistic") { "On" } else { "Off" };
    group.append(&ui::row("Encrypted DNS", Some("DNS over TLS"), &value(tls)));
    group.append(&ui::row("Changing DNS", Some("Edit /etc/systemd/resolved.conf as root; it isn't editable from here"), &value("")));
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
