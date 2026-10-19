//! Users & Accounts: who has an account, and changing them: the full name, administrator or not, the shell, the
//! password, adding and removing accounts. Every change is root's, so it goes through the helper with your password (see
//! `privileged.rs`); the helper refuses what would lock everyone out (removing yourself or the last administrator).
//! Your own password is changed on Fingerprint & Lock, which can do it without root.

use std::rc::Rc;

use gtk::prelude::*;
use zeroize::Zeroizing;

use crate::{privileged, ui};

struct Account {
    name: String,
    full_name: String,
    shell: String,
    home: String,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Users & Accounts", Some("Changing an account asks for your password.")));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fill(&body, &status);
    column.append(&body);
    column.append(&status);
    page.upcast()
}

fn fill(body: &gtk::Box, status: &gtk::Label) {
    ui::clear(body);
    let me = std::env::var("USER").unwrap_or_default();
    let admins = administrators();
    let shells = shells();
    let accounts = accounts();
    let mut ordered: Vec<&Account> = accounts.iter().filter(|account| account.name == me).collect();
    ordered.extend(accounts.iter().filter(|account| account.name != me));
    for account in ordered {
        let yours = account.name == me;
        let group = ui::group();
        let refresh = {
            let (body, status) = (body.clone(), status.clone());
            Rc::new(move || fill(&body, &status))
        };

        let name_field = ui::field(&account.full_name, 24, {
            let (name, old, status, refresh) = (account.name.clone(), account.full_name.clone(), status.clone(), refresh.clone());
            move |text| {
                let text = text.trim();
                if text == old {
                    return true;
                }
                if text.len() > 60 || text.chars().any(|c| c.is_control() || matches!(c, ':' | ',' | '=' | '\\' | '"')) {
                    return false;
                }
                let refresh = refresh.clone();
                privileged::apply(&status.clone(), "change the name", &["user-fullname", &name, text], None, &status, move |_| refresh());
                true
            }
        });
        name_field.set_placeholder_text(Some(&account.name));
        group.append(&ui::row("Full name", Some(&format!("Login name: {}", account.name)), &name_field));

        let admin = ui::switch(admins.contains(&account.name), {
            let (name, status, refresh) = (account.name.clone(), status.clone(), refresh.clone());
            move |on| {
                let refresh = refresh.clone();
                privileged::apply(&status.clone(), "change what the account can do", &["user-admin", &name, if on { "yes" } else { "no" }], None, &status, move |_| refresh());
            }
        });
        admin.set_sensitive(!yours);
        group.append(&ui::row("Administrator", Some(if yours { "You can't change your own while logged in" } else { "Can use su and sudo, and change other accounts" }), &admin));

        let options: Vec<(&str, &str)> = shells.iter().map(|shell| (shell.as_str(), shell.as_str())).collect();
        group.append(&ui::row(
            "Shell",
            None,
            &ui::choice(&options, &account.shell, {
                let (name, status) = (account.name.clone(), status.clone());
                move |shell| privileged::apply(&status.clone(), "change the shell", &["user-shell", &name, shell], None, &status, |_| {})
            }),
        ));

        group.append(&ui::row("Home folder", None, &ui::value(&account.home)));

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if yours {
            actions.append(&ui::button("Change my password…", |button| {
                let _ = button.activate_action("win.show-pane", Some(&"lock".to_variant()));
            }));
        } else {
            actions.append(&ui::button("Set a new password…", {
                let (name, status) = (account.name.clone(), status.clone());
                move |button| {
                    let (name, status) = (name.clone(), status.clone());
                    let parent = button.root().and_downcast::<gtk::Window>();
                    form(parent.as_ref(), &format!("New password for {name}"), "", &[("New password", true), ("Again", true)], None, "Change password", {
                        let parent = parent.clone();
                        move |values, _| {
                            check_password(&values[0], &values[1])?;
                            let widget = status.clone();
                            let _ = &parent;
                            privileged::apply(&widget, "change the password", &["user-password", &name], Some(Zeroizing::new(values[0].clone())), &status, |_| {});
                            Ok(())
                        }
                    });
                }
            }));
            actions.append(&ui::button("Remove account…", {
                let (name, status, refresh) = (account.name.clone(), status.clone(), refresh.clone());
                move |button| {
                    let (name, status, refresh) = (name.clone(), status.clone(), refresh.clone());
                    let parent = button.root().and_downcast::<gtk::Window>();
                    form(parent.as_ref(), &format!("Remove {name}?"), "Their login stops working at once. This can't be undone if you also delete their files.", &[], Some(("Also delete their home folder and files", false)), "Remove", move |_, remove_home| {
                        let refresh = refresh.clone();
                        privileged::apply(&status.clone(), "remove the account", &["user-remove", &name, if remove_home { "remove-home" } else { "keep-home" }], None, &status, move |_| refresh());
                        Ok(())
                    });
                }
            }));
        }
        group.append(&ui::row("Password and account", None, &actions));
        body.append(&ui::section(&if yours { format!("You · {}", account.name) } else { account.name.clone() }, None, &group));
    }

    let add = ui::button("Add an account…", {
        let (status, body) = (status.clone(), body.clone());
        move |button| {
            let (status, body) = (status.clone(), body.clone());
            let parent = button.root().and_downcast::<gtk::Window>();
            form(
                parent.as_ref(),
                "Add an account",
                "Login names are lower-case letters, digits, - and _, starting with a letter.",
                &[("Login name", false), ("Full name (optional)", false), ("Password", true), ("Again", true)],
                Some(("Administrator", false)),
                "Add account",
                move |values, admin| {
                    let (login, full) = (values[0].trim().to_owned(), values[1].trim().to_owned());
                    let mut chars = login.chars();
                    let fine = chars.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_') && login.len() <= 32 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
                    if !fine {
                        return Err("That isn't a good login name: lower-case letters, digits, - and _, starting with a letter.".to_owned());
                    }
                    if full.len() > 60 || full.chars().any(|c| c.is_control() || matches!(c, ':' | ',' | '=' | '\\' | '"')) {
                        return Err("The full name can't have colons, commas, quotes or equals signs.".to_owned());
                    }
                    check_password(&values[2], &values[3])?;
                    let (status_after, body_after, login_after) = (status.clone(), body.clone(), login.clone());
                    privileged::apply(&status.clone(), "add the account", &["user-add", &login, if admin { "yes" } else { "no" }], Some(Zeroizing::new(values[2].clone())), &status, move |_| {
                        if full.is_empty() {
                            fill(&body_after, &status_after);
                            return;
                        }
                        let (status, body) = (status_after.clone(), body_after.clone());
                        privileged::apply(&status_after.clone(), "name the account", &["user-fullname", &login_after, &full], None, &status_after, move |_| fill(&body, &status));
                    });
                    Ok(())
                },
            );
        }
    });
    add.set_halign(gtk::Align::Start);
    body.append(&add);
}

fn check_password(first: &str, again: &str) -> Result<(), String> {
    if first.is_empty() {
        return Err("Type a password.".to_owned());
    }
    if first != again {
        return Err("The two passwords aren't the same.".to_owned());
    }
    if first.contains(['\n', '\r']) {
        return Err("A password can't have a line break.".to_owned());
    }
    Ok(())
}

/// A small dialog: `fields` are (label, hidden), `toggle` is an optional switch (label, starting value); `submit` gets the
/// texts and the switch and either closes it (`Ok`) or says what's wrong in the dialog (`Err`).
fn form(parent: Option<&gtk::Window>, title: &str, note: &str, fields: &[(&str, bool)], toggle: Option<(&str, bool)>, ok: &str, submit: impl Fn(Vec<String>, bool) -> Result<(), String> + 'static) {
    let window = gtk::Window::builder().title(title).modal(true).resizable(false).default_width(380).build();
    window.set_transient_for(parent);
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("settings");
    window.add_css_class("dialog");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.add_css_class("page");
    let heading = gtk::Label::builder().label(title).xalign(0.0).build();
    heading.add_css_class("title");
    column.append(&heading);
    if !note.is_empty() {
        let note = ui::note(note);
        note.set_wrap(true);
        column.append(&note);
    }
    let mut entries: Vec<gtk::Editable> = Vec::new();
    for (label, hidden) in fields {
        let line = gtk::Box::new(gtk::Orientation::Vertical, 4);
        line.append(&ui::note(label));
        if *hidden {
            let entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
            entry.add_css_class("field");
            line.append(&entry);
            entries.push(entry.upcast());
        } else {
            let entry = gtk::Entry::new();
            entry.add_css_class("field");
            line.append(&entry);
            entries.push(entry.upcast());
        }
        column.append(&line);
    }
    let switch = toggle.map(|(label, on)| {
        let switch = ui::switch(on, |_| {});
        column.append(&ui::row(label, None, &switch));
        switch
    });
    let error = ui::note("");
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_visible(false);
    column.append(&error);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let go = {
        let (window, entries, switch, error) = (window.downgrade(), entries.clone(), switch.clone(), error.clone());
        Rc::new(move || {
            let values: Vec<String> = entries.iter().map(|entry| entry.text().to_string()).collect();
            match submit(values, switch.as_ref().is_some_and(|switch| switch.is_active())) {
                Ok(()) => {
                    if let Some(window) = window.upgrade() {
                        window.close();
                    }
                }
                Err(text) => {
                    error.set_label(&text);
                    error.set_visible(true);
                }
            }
        })
    };
    let cancel = ui::button("Cancel", {
        let window = window.downgrade();
        move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    let confirm = ui::button(ok, {
        let go = go.clone();
        move |_| go()
    });
    confirm.add_css_class("suggested");
    if let Some(last) = entries.last() {
        let go = go.clone();
        if let Some(entry) = last.dynamic_cast_ref::<gtk::Entry>() {
            entry.connect_activate(move |_| go());
        } else if let Some(entry) = last.dynamic_cast_ref::<gtk::PasswordEntry>() {
            entry.connect_activate(move |_| go());
        }
    }
    buttons.append(&cancel);
    buttons.append(&confirm);
    column.append(&buttons);
    window.set_child(Some(&column));
    window.present();
}

fn shells() -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_to_string("/etc/shells").unwrap_or_default().lines().filter(|line| line.starts_with('/') && std::path::Path::new(line).is_file()).map(str::to_owned).collect();
    found.dedup();
    found
}

fn administrators() -> Vec<String> {
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let line = group.lines().find(|line| line.starts_with("wheel:")).unwrap_or("");
    line.split(':').nth(3).unwrap_or("").split(',').filter(|name| !name.is_empty()).map(str::to_owned).collect()
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
            (uid >= 1000 && uid < 65000 && name != "nobody").then(|| Account { name: name.to_owned(), full_name: full_name.split(',').next().unwrap_or("").to_owned(), shell: shell.to_owned(), home: home.to_owned() })
        })
        .collect()
}
