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
/// process at a time where that isn't allowed or the cgroup holds other programs too.
pub fn kill(app: &App) -> Result<(), String> {
    let mine: BTreeSet<u32> = app.processes.iter().map(|process| process.pid).collect();
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
        let dir = Path::new("/sys/fs/cgroup").join(cgroup.trim_start_matches('/'));
        // The login session's cgroup holds the whole desktop; only a cgroup that is all this app's goes at once.
        let alone = std::fs::read_to_string(dir.join("cgroup.procs")).is_ok_and(|procs| procs.lines().filter_map(|line| line.trim().parse::<u32>().ok()).all(|pid| mine.contains(&pid)));
        if !alone || std::fs::write(dir.join("cgroup.kill"), "1").is_err() {
            left.push(cgroup.clone());
        }
    }
    let mut pids: BTreeSet<u32> = app
        .processes
        .iter()
        .filter(|process| {
            std::fs::read_to_string(format!("/proc/{}/cgroup", process.pid))
                .is_ok_and(|text| left.iter().any(|cgroup| text.lines().any(|line| line.strip_prefix("0::") == Some(cgroup.as_str()))))
        })
        .map(|process| process.pid)
        .collect();
    if pids.is_empty() {
        return Ok(());
    }
    // What the app started since the last reading would otherwise survive it.
    add_descendants(&mut pids);
    let args: Vec<String> = pids.iter().map(u32::to_string).collect();
    let status = std::process::Command::new("kill").arg("-KILL").args(&args).status().map_err(|err| err.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("kill exited with {status}")) }
}

/// Adds every process now running below the ones given.
fn add_descendants(pids: &mut BTreeSet<u32>) {
    let parents: Vec<(u32, u32)> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let parent = stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()?;
            Some((pid, parent))
        })
        .collect();
    loop {
        let before = pids.len();
        let below: Vec<u32> = parents.iter().filter(|(_, parent)| pids.contains(parent)).map(|(pid, _)| *pid).collect();
        pids.extend(below);
        if pids.len() == before {
            return;
        }
    }
}

/// Apps worth offering: ones the user started, through systemd's `app.slice` or from the login
/// session (a launcher, a key binding), not the desktop's own services or this app.
pub fn quittable(apps: &[App]) -> Vec<App> {
    let me = std::process::id();
    apps.iter()
        .filter(|app| (app.id.contains("/app.slice/") || app.id.contains('#')) && !app.system && !app.processes.iter().any(|process| process.pid == me))
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

#[cfg(test)]
mod tests {
    use super::*;
    use autark_common::stats::Process;

    #[test]
    fn kills_the_programs_tree_and_leaves_the_cgroup_alone() {
        // A program with a helper, sharing this test's cgroup, as launcher-started apps share the login session's.
        let mut parent = std::process::Command::new("sh").args(["-c", "sleep 60 & sleep 60 & wait"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let pid = parent.id();
        // The list was read before the helpers started, so only the parent is known.
        let app = App { id: "/session.scope#1".to_owned(), processes: vec![Process { pid, ..Default::default() }], ..Default::default() };
        kill(&app).unwrap();
        let status = parent.wait().unwrap();
        assert!(!status.success());
        std::thread::sleep(std::time::Duration::from_millis(200));
        let alive = std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            std::fs::read_to_string(entry.path().join("stat")).is_ok_and(|stat| stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().nth(1).map(|p| p == pid.to_string())).unwrap_or(false))
        });
        assert!(!alive, "a helper of the killed program is still running");
    }

    #[test]
    fn offers_session_programs_but_not_the_desktop() {
        let app = |id: &str, system| App { id: id.to_owned(), system, ..Default::default() };
        let apps = [
            app("/user.slice/user-1000.slice/session-1.scope#2042", false),
            app("/user.slice/user-1000.slice/session-1.scope#1130", true),
            app("/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope", false),
            app("/user.slice/user-1000.slice/user@1000.service/session.slice/pipewire.service", false),
        ];
        let ids: Vec<String> = quittable(&apps).into_iter().map(|app| app.id).collect();
        assert_eq!(ids, ["/user.slice/user-1000.slice/session-1.scope#2042", "/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope"]);
    }
}
