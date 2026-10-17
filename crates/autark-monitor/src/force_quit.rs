//! Force quit: the dialog (also opened from anywhere with Ctrl+Alt+Esc, which runs
//! `autark-monitor --force-quit`) and the kill itself.

use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use autark_common::stats::App;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::window::app_icon;

/// Kills every process of an app at once: through each of its cgroups' `cgroup.kill`, or one
/// process at a time where that isn't allowed.
pub fn kill(app: &App) -> Result<(), String> {
    let mut cgroups: BTreeSet<String> = BTreeSet::new();
    for process in &app.processes {
        if let Ok(text) = std::fs::read_to_string(format!("/proc/{}/cgroup", process.pid))
            && let Some(path) = text.lines().find_map(|line| line.strip_prefix("0::"))
        {
            cgroups.insert(path.to_owned());
        }
    }
    let mut left = Vec::new();
    for cgroup in &cgroups {
        let file = Path::new("/sys/fs/cgroup").join(cgroup.trim_start_matches('/')).join("cgroup.kill");
        if std::fs::write(&file, "1").is_err() {
            left.push(cgroup.clone());
        }
    }
    let pids: Vec<String> = app
        .processes
        .iter()
        .filter(|process| {
            std::fs::read_to_string(format!("/proc/{}/cgroup", process.pid))
                .is_ok_and(|text| left.iter().any(|cgroup| text.lines().any(|line| line.strip_prefix("0::") == Some(cgroup.as_str()))))
        })
        .map(|process| process.pid.to_string())
        .collect();
    if pids.is_empty() {
        return Ok(());
    }
    let status = std::process::Command::new("kill").arg("-KILL").args(&pids).status().map_err(|err| err.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("kill exited with {status}")) }
}

/// Apps worth offering: ones the user started, not the desktop's own services or this app.
pub fn quittable(apps: &[App]) -> Vec<App> {
    let me = std::process::id();
    apps.iter()
        .filter(|app| app.id.contains("/app.slice/") && !app.processes.iter().any(|process| process.pid == me))
        .cloned()
        .collect()
}

/// Shows the dialog. `flagged` says which apps get a "High CPU" note.
pub fn dialog(app: &gtk::Application, parent: Option<&gtk::Window>, apps: Vec<App>, flagged: impl Fn(&str) -> bool, select: Option<&str>) {
    // Belonging to the app keeps it running while the dialog is open on its own.
    let window = gtk::Window::builder().application(app).title("Force quit").default_width(420).resizable(false).modal(parent.is_some()).build();
    window.set_transient_for(parent);
    window.add_css_class("force-quit");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let title = gtk::Label::builder().label("Force quit an app").xalign(0.0).build();
    title.add_css_class("title");
    let note = gtk::Label::builder().label("Unsaved changes in the app are lost. Ctrl+Alt+Esc opens this from anywhere.").xalign(0.0).wrap(true).build();
    note.add_css_class("note");
    let head = gtk::Box::new(gtk::Orientation::Vertical, 4);
    head.add_css_class("head");
    head.append(&title);
    head.append(&note);

    let list = gtk::ListBox::new();
    list.add_css_class("apps");
    for app in &apps {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&app_icon(app, 24));
        let name = gtk::Label::builder().label(&app.name).xalign(0.0).hexpand(true).build();
        row.append(&name);
        if flagged(&app.id) {
            let flag = gtk::Label::new(Some("High CPU"));
            flag.add_css_class("flag");
            row.append(&flag);
        }
        list.append(&row);
    }
    let empty = gtk::Label::new(Some("No apps are running"));
    empty.add_css_class("note");
    list.set_placeholder(Some(&empty));

    let cancel = gtk::Button::with_label("Cancel");
    let quit = gtk::Button::with_label("Force quit");
    quit.add_css_class("destructive");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.add_css_class("buttons");
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&quit);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&gtk::WindowHandle::builder().child(&head).build());
    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).max_content_height(380).child(&list).build();
    content.append(&scroller);
    content.append(&buttons);
    window.set_child(Some(&content));

    let apps = Rc::new(apps);
    let selected = select.and_then(|id| apps.iter().position(|app| app.id == id)).unwrap_or(0);
    if let Some(row) = list.row_at_index(selected as i32) {
        list.select_row(Some(&row));
    }
    let update_label = {
        let (apps, quit) = (apps.clone(), quit.clone());
        move |list: &gtk::ListBox| {
            let app = list.selected_row().and_then(|row| apps.get(row.index() as usize));
            quit.set_label(&app.map_or("Force quit".to_owned(), |app| format!("Force quit {}", app.name)));
            quit.set_sensitive(app.is_some());
        }
    };
    update_label(&list);
    list.connect_selected_rows_changed(update_label);

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    let (w, list_ref) = (window.clone(), list.clone());
    quit.connect_clicked(move |_| {
        let Some(app) = list_ref.selected_row().and_then(|row| apps.get(row.index() as usize)) else { return };
        if let Err(err) = kill(app) {
            eprintln!("monitor: force quitting {}: {err}", app.name);
        }
        w.close();
    });
    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
}
