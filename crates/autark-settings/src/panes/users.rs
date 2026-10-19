//! Users & Accounts: who you're logged in as, what that account can do, and who else has one, read from the system.
//! Adding or removing accounts is root's work, so it's not offered here. Your password and fingerprints are on the
//! Fingerprint & Lock pane.

use std::process::Command;

use gtk::prelude::*;

use crate::ui;

struct Account {
    name: String,
    full_name: String,
    home: String,
    shell: String,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Users & Accounts", Some("Your password and fingerprints are on Fingerprint & Lock.")));
    let me = std::env::var("USER").unwrap_or_default();
    let accounts = accounts();
    let groups: Vec<String> = text(&["id", "-Gn"]).split_whitespace().map(str::to_owned).collect();

    if let Some(account) = accounts.iter().find(|account| account.name == me) {
        let group = ui::group();
        group.append(&ui::row("Name", None, &ui::value(if account.full_name.is_empty() { &account.name } else { &account.full_name })));
        group.append(&ui::row("Username", None, &ui::value(&account.name)));
        let kind = if groups.iter().any(|group| group == "wheel" || group == "sudo") { "Administrator" } else { "Standard" };
        group.append(&ui::row("Account type", Some("Administrators can use su and sudo"), &ui::value(kind)));
        group.append(&ui::row("Home folder", None, &ui::value(&account.home)));
        group.append(&ui::row("Shell", None, &ui::value(&account.shell)));
        group.append(&ui::row("Groups", None, &ui::value(&groups.join(", "))));
        column.append(&ui::section("You", None, &group));
    }

    let others: Vec<&Account> = accounts.iter().filter(|account| account.name != me).collect();
    if !others.is_empty() {
        let group = ui::group();
        for account in others {
            group.append(&ui::row(if account.full_name.is_empty() { &account.name } else { &account.full_name }, Some(&account.name), &ui::value(&account.home)));
        }
        column.append(&ui::section("Other accounts", None, &group));
    }
    column.append(&ui::note("Adding, removing or changing the type of an account needs root: use useradd and usermod in a root shell."));
    page.upcast()
}

fn text(command: &[&str]) -> String {
    Command::new(command[0]).args(&command[1..]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
}

/// The people's accounts in `/etc/passwd` (user IDs from 1000, apart from `nobody`).
fn accounts() -> Vec<Account> {
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    passwd
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let [name, _, uid, _, full_name, home, shell] = fields[..] else { return None };
            let uid: u32 = uid.parse().ok()?;
            (uid >= 1000 && uid < 65000 && name != "nobody").then(|| Account { name: name.to_owned(), full_name: full_name.split(',').next().unwrap_or("").to_owned(), home: home.to_owned(), shell: shell.to_owned() })
        })
        .collect()
}
