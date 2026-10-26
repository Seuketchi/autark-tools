//! The Activity window: apps and their processes, measured five ways, with a ten-minute chart
//! and a summary along the bottom.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use autark_common::stats::{App, Snapshot};
use autark_common::theme;
use gtk::prelude::*;
use gtk::{gio, pango};

use crate::stats::{Event, History, Sample};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Cpu,
    Memory,
    Energy,
    Disk,
    Network,
}

const TABS: [(Tab, &str); 5] = [(Tab::Cpu, "CPU"), (Tab::Memory, "Memory"), (Tab::Energy, "Energy"), (Tab::Disk, "Disk"), (Tab::Network, "Network")];

/// One measurement in a row: the text, a width, and an optional bar from 0 to 100.
struct Measure {
    text: String,
    width: i32,
    bar: Option<(f64, bool)>,
}

pub struct Monitor {
    pub window: RefCell<Option<gtk::ApplicationWindow>>,
    app: gtk::Application,
    snapshot: RefCell<Option<Snapshot>>,
    history: RefCell<History>,
    problem: RefCell<Option<String>>,
    tab: Cell<Tab>,
    selected: RefCell<Option<String>>,
    expanded: RefCell<HashSet<String>>,
    widgets: RefCell<Option<Widgets>>,
}

struct Widgets {
    subtitle: gtk::Label,
    search: gtk::SearchEntry,
    first_column: gtk::Label,
    columns: gtk::Box,
    rows: gtk::Box,
    chart_title: gtk::Label,
    chart_caption: gtk::Label,
    chart: gtk::DrawingArea,
    cards: gtk::Box,
}

impl Monitor {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let monitor = Rc::new(Self {
            window: RefCell::default(),
            app: app.clone(),
            snapshot: RefCell::default(),
            history: RefCell::default(),
            problem: RefCell::default(),
            tab: Cell::new(Tab::Energy),
            selected: RefCell::default(),
            expanded: RefCell::default(),
            widgets: RefCell::default(),
        });
        let weak = Rc::downgrade(&monitor);
        crate::stats::follow(move |event| {
            let Some(monitor) = weak.upgrade() else { return };
            match event {
                Event::Snapshot(snapshot) => {
                    monitor.history.borrow_mut().add(&snapshot);
                    monitor.snapshot.replace(Some(*snapshot));
                    monitor.problem.replace(None);
                }
                Event::Unavailable(problem) => {
                    monitor.problem.replace(Some(problem));
                }
            }
            monitor.refresh();
        });
        monitor
    }

    pub fn has_snapshot(&self) -> bool {
        self.snapshot.borrow().is_some()
    }

    pub fn show_window(self: &Rc<Self>) {
        if let Some(window) = &*self.window.borrow() {
            window.present();
            return;
        }
        let window = self.build();
        window.present();
        self.window.replace(Some(window));
        self.refresh();
    }

    /// The force-quit dialog, over the window if it's open.
    pub fn show_force_quit(self: &Rc<Self>) {
        let apps = self.snapshot.borrow().as_ref().map(|snapshot| crate::force_quit::quittable(&snapshot.apps)).unwrap_or_default();
        let window = self.window.borrow().clone().map(|window| window.upcast::<gtk::Window>());
        let weak = Rc::downgrade(self);
        let flagged = move |id: &str| weak.upgrade().is_some_and(|monitor| monitor.history.borrow().flag(id).is_some());
        crate::force_quit::dialog(&self.app, window.as_ref(), apps, flagged, self.selected.borrow().as_deref());
    }

    fn build(self: &Rc<Self>) -> gtk::ApplicationWindow {
        let window = gtk::ApplicationWindow::builder().application(&self.app).title("Activity").default_width(1240).default_height(720).build();
        window.add_css_class("activity");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let controls = autark_common::chrome::controls(true);
        let title = gtk::Label::builder().label("Activity").xalign(0.0).build();
        title.add_css_class("title");
        let subtitle = gtk::Label::builder().xalign(0.0).build();
        subtitle.add_css_class("subtitle");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 1);
        titles.set_size_request(150, -1);
        titles.append(&title);
        titles.append(&subtitle);

        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("tabs");
        tabs.set_hexpand(true);
        tabs.set_halign(gtk::Align::Center);
        let mut first: Option<gtk::ToggleButton> = None;
        for (tab, label) in TABS {
            let button = gtk::ToggleButton::builder().label(label).active(tab == self.tab.get()).build();
            button.set_group(first.as_ref());
            first.get_or_insert(button.clone());
            let weak = Rc::downgrade(self);
            button.connect_toggled(move |button| {
                if let Some(monitor) = weak.upgrade()
                    && button.is_active()
                {
                    monitor.tab.set(tab);
                    monitor.refresh();
                }
            });
            tabs.append(&button);
        }

        let search = gtk::SearchEntry::builder().placeholder_text("Search").width_request(200).build();
        let force_quit = gtk::Button::builder().build();
        let force_label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        force_label.append(&gtk::Image::from_icon_name("process-stop-symbolic"));
        force_label.append(&gtk::Label::new(Some("Force quit")));
        force_quit.set_child(Some(&force_label));
        force_quit.add_css_class("force-quit-button");
        force_quit.set_tooltip_text(Some("Force quit an app (Ctrl+Alt+Esc)"));

        for widget in [controls.upcast_ref::<gtk::Widget>(), tabs.upcast_ref(), search.upcast_ref(), force_quit.upcast_ref()] {
            widget.set_valign(gtk::Align::Center);
        }
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        header.add_css_class("header");
        header.append(&titles);
        header.append(&tabs);
        header.append(&search);
        header.append(&force_quit);
        header.append(&controls);

        let first_column = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let column_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        column_row.add_css_class("columns");
        column_row.append(&first_column);
        column_row.append(&columns);
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rows.add_css_class("rows");
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&rows).build();

        let chart_title = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        chart_title.add_css_class("card-title");
        let chart_caption = gtk::Label::new(None);
        chart_caption.add_css_class("caption");
        let chart_head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        chart_head.append(&chart_title);
        chart_head.append(&chart_caption);
        let chart = gtk::DrawingArea::builder().vexpand(true).build();
        let chart_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        chart_card.add_css_class("card");
        chart_card.set_hexpand(true);
        chart_card.append(&chart_head);
        chart_card.append(&chart);
        let cards = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        cards.set_homogeneous(true);
        cards.set_hexpand(true);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        footer.add_css_class("footer");
        // The chart fills the footer, but the footer keeps its height; the list gets the rest.
        footer.set_vexpand(false);
        footer.set_size_request(-1, 168);
        footer.set_homogeneous(true);
        footer.append(&chart_card);
        footer.append(&cards);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&gtk::WindowHandle::builder().child(&header).build());
        content.append(&column_row);
        content.append(&scroller);
        content.append(&footer);
        window.set_child(Some(&content));

        let weak = Rc::downgrade(self);
        chart.set_draw_func(move |_, cr, width, height| {
            if let Some(monitor) = weak.upgrade() {
                monitor.draw_chart(cr, width as f64, height as f64);
            }
        });
        let weak = Rc::downgrade(self);
        search.connect_search_changed(move |_| {
            if let Some(monitor) = weak.upgrade() {
                monitor.refresh();
            }
        });
        let weak = Rc::downgrade(self);
        force_quit.connect_clicked(move |_| {
            if let Some(monitor) = weak.upgrade() {
                monitor.show_force_quit();
            }
        });
        let weak = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(monitor) = weak.upgrade() {
                monitor.window.replace(None);
                monitor.widgets.replace(None);
            }
            gtk::glib::Propagation::Proceed
        });
        self.widgets.replace(Some(Widgets { subtitle, search, first_column, columns, rows, chart_title, chart_caption, chart, cards }));
        window
    }

    fn refresh(self: &Rc<Self>) {
        let widgets = self.widgets.borrow();
        let Some(w) = widgets.as_ref() else { return };
        let snapshot = self.snapshot.borrow();
        let tab = self.tab.get();

        clear(&w.rows);
        clear(&w.columns);
        clear(&w.cards);
        let Some(snapshot) = snapshot.as_ref() else {
            let text = self.problem.borrow().clone().unwrap_or_else(|| "Waiting for autark-stats…".to_owned());
            let label = gtk::Label::new(Some(&text));
            label.add_css_class("empty");
            w.rows.append(&label);
            return;
        };
        let processes: usize = snapshot.apps.iter().map(|app| app.processes.len()).sum();
        w.subtitle.set_label(&format!("{} apps  ·  {processes} processes", snapshot.apps.len()));

        let columns: &[(&str, i32)] = match tab {
            Tab::Cpu => &[("% CPU", 110), ("Processes", 100), ("Memory", 110)],
            Tab::Memory => &[("Memory", 120), ("% of RAM", 100), ("Processes", 100)],
            Tab::Energy => &[("Energy impact", 170), ("Power", 90), ("% CPU", 90)],
            Tab::Disk => &[("Read", 120), ("Written", 120), ("Processes", 100)],
            Tab::Network => &[("Received", 130), ("Sent", 130)],
        };
        w.first_column.set_label(if tab == Tab::Network { "INTERFACE" } else { "APP" });
        for (label, width) in columns {
            let header = gtk::Label::builder().label(label.to_uppercase()).xalign(1.0).width_request(*width).build();
            w.columns.append(&header);
        }

        if tab == Tab::Network {
            let cells = vec![
                Measure { text: rate(snapshot.network.read as f64), width: 130, bar: None },
                Measure { text: rate(snapshot.network.write as f64), width: 130, bar: None },
            ];
            w.rows.append(&self.row(None, "All network interfaces", None, cells, false));
            let note = gtk::Label::builder().label("Per-app network needs a kernel probe; it comes after the first release.").xalign(0.0).build();
            note.add_css_class("note");
            w.rows.append(&note);
        } else {
            let query = w.search.text().to_lowercase();
            let total_cpu: f64 = snapshot.apps.iter().map(|app| app.cpu).sum();
            let impact = |app: &App| match (&snapshot.power, app.watts) {
                (Some(power), Some(watts)) if power.watts > 0.0 => watts / power.watts * 100.0,
                _ if total_cpu > 0.0 => app.cpu / total_cpu * 100.0,
                _ => 0.0,
            };
            let mut apps: Vec<&App> = snapshot
                .apps
                .iter()
                .filter(|app| query.is_empty() || app.name.to_lowercase().contains(&query) || app.processes.iter().any(|p| p.name.to_lowercase().contains(&query)))
                .collect();
            let key = |app: &App| match tab {
                Tab::Cpu => app.cpu,
                Tab::Memory => app.memory as f64,
                Tab::Energy => impact(app),
                _ => (app.disk.read + app.disk.write) as f64,
            };
            apps.sort_by(|a, b| key(b).total_cmp(&key(a)));
            let total_memory = snapshot.memory.total.max(1) as f64;
            for app in apps {
                let cells = match tab {
                    Tab::Cpu => vec![cell(format!("{:.1}", app.cpu), 110), cell(app.processes.len().to_string(), 100), cell(bytes(app.memory), 110)],
                    Tab::Memory => vec![cell(bytes(app.memory), 120), cell(format!("{:.1}%", app.memory as f64 * 100.0 / total_memory), 100), cell(app.processes.len().to_string(), 100)],
                    Tab::Energy => {
                        let value = impact(app);
                        vec![
                            Measure { text: format!("{value:.1}"), width: 170, bar: Some((value, value > 40.0)) },
                            cell(app.watts.map_or("—".to_owned(), |watts| format!("{watts:.1} W")), 90),
                            cell(format!("{:.1}", app.cpu), 90),
                        ]
                    }
                    _ => vec![cell(rate(app.disk.read as f64), 120), cell(rate(app.disk.write as f64), 120), cell(app.processes.len().to_string(), 100)],
                };
                let open = self.expanded.borrow().contains(&app.id);
                let flag = self.history.borrow().flag(&app.id);
                w.rows.append(&self.row(Some(app), &app.name, flag.as_deref(), cells, open));
                if open {
                    for process in &app.processes {
                        let cells = match tab {
                            Tab::Cpu => vec![cell(format!("{:.1}", process.cpu), 110), cell(String::new(), 100), cell(bytes(process.memory), 110)],
                            Tab::Memory => vec![cell(bytes(process.memory), 120), cell(format!("{:.1}%", process.memory as f64 * 100.0 / total_memory), 100), cell(String::new(), 100)],
                            Tab::Energy => {
                                let share = if app.cpu > 0.0 { process.cpu / app.cpu } else { 0.0 };
                                let value = impact(app) * share;
                                vec![
                                    Measure { text: format!("{value:.1}"), width: 170, bar: Some((value, false)) },
                                    cell(app.watts.map_or(String::new(), |watts| format!("{:.1} W", watts * share)), 90),
                                    cell(format!("{:.1}", process.cpu), 90),
                                ]
                            }
                            _ => vec![cell(String::new(), 120), cell(String::new(), 120), cell(String::new(), 100)],
                        };
                        w.rows.append(&process_row(&process.name, process.pid, cells));
                        if tab == Tab::Cpu {
                            for thread in &process.threads {
                                let cells = vec![cell(format!("{:.1}", thread.cpu), 110), cell(String::new(), 100), cell(String::new(), 110)];
                                w.rows.append(&process_row(&format!("      ↳ {}", thread.name), thread.tid, cells));
                            }
                        }
                    }
                }
            }
        }

        let (title, caption) = self.chart_text(snapshot);
        w.chart_title.set_label(&title);
        w.chart_caption.set_label(&caption);
        w.chart.queue_draw();
        for (title, big, lines, meter) in self.cards(snapshot) {
            w.cards.append(&card(&title, &big, &lines, meter));
        }
    }

    fn row(self: &Rc<Self>, app: Option<&App>, name: &str, flag: Option<&str>, cells: Vec<Measure>, open: bool) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("row");
        let id = app.map(|app| app.id.clone());
        if id.is_some() && *self.selected.borrow() == id {
            row.add_css_class("selected");
        }
        if let Some(app) = app {
            let chevron = gtk::Button::from_icon_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" });
            chevron.add_css_class("chevron");
            chevron.set_sensitive(!app.processes.is_empty());
            let (weak, id) = (Rc::downgrade(self), app.id.clone());
            chevron.connect_clicked(move |_| {
                if let Some(monitor) = weak.upgrade() {
                    let mut expanded = monitor.expanded.borrow_mut();
                    if !expanded.remove(&id) {
                        expanded.insert(id.clone());
                    }
                    drop(expanded);
                    monitor.refresh();
                }
            });
            row.append(&chevron);
            row.append(&app_icon(app, 22));
        }
        let name_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        name_box.set_hexpand(true);
        let label = gtk::Label::builder().label(name).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        label.add_css_class("name");
        name_box.append(&label);
        if let Some(flag) = flag {
            let chip = gtk::Label::new(Some(flag));
            chip.add_css_class("flag");
            name_box.append(&chip);
        }
        row.append(&name_box);
        append_cells(&row, cells);
        if let Some(id) = id {
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(self);
            click.connect_released(move |_, _, _, _| {
                if let Some(monitor) = weak.upgrade() {
                    monitor.selected.replace(Some(id.clone()));
                    monitor.refresh();
                }
            });
            row.add_controller(click);
        }
        row
    }

    /// The series the chart shows for the current tab, and a second line for disk and network.
    fn series(&self) -> (Vec<f64>, Option<Vec<f64>>, Option<f64>) {
        let history = self.history.borrow();
        let take = |f: fn(&Sample) -> f64| history.samples.iter().map(f).collect::<Vec<f64>>();
        match self.tab.get() {
            Tab::Cpu => (take(|s| s.cpu), None, Some(100.0)),
            Tab::Memory => (take(|s| s.memory), None, Some(100.0)),
            Tab::Energy => (take(|s| s.watts.unwrap_or(0.0)), None, None),
            Tab::Disk => (take(|s| s.disk_read), Some(take(|s| s.disk_write)), None),
            Tab::Network => (take(|s| s.received), Some(take(|s| s.sent)), None),
        }
    }

    fn draw_chart(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let (values, second, fixed) = self.series();
        if values.len() < 2 {
            return;
        }
        let peak = values.iter().chain(second.iter().flatten()).copied().fold(0.0, f64::max);
        let max = fixed.unwrap_or((peak * 1.15).max(1e-9));
        let point = |i: usize, count: usize, v: f64| (i as f64 / (count - 1) as f64 * width, height - 2.0 - (v / max).min(1.0) * (height - 4.0));
        let trace = |series: &[f64]| {
            for (i, &v) in series.iter().enumerate() {
                let (x, y) = point(i, series.len(), v);
                if i == 0 { cr.move_to(x, y) } else { cr.line_to(x, y) }
            }
        };
        trace(&values);
        cr.line_to(width, height);
        cr.line_to(0.0, height);
        cr.close_path();
        let palette = &theme::look().palette;
        theme::set_source(cr, &palette.accent, 0.14);
        let _ = cr.fill();
        cr.set_line_width(2.0);
        trace(&values);
        theme::set_source(cr, &palette.accent, 1.0);
        let _ = cr.stroke();
        if let Some(second) = second {
            trace(&second);
            theme::set_source(cr, &palette.warning, 1.0);
            let _ = cr.stroke();
        }
    }

    fn chart_text(&self, snapshot: &Snapshot) -> (String, String) {
        let minutes = (self.history.borrow().samples.len() as f64 / 60.0).ceil().max(1.0) as u64;
        let span = if minutes >= 10 { "LAST 10 MINUTES".to_owned() } else { format!("LAST {minutes} MIN") };
        let watts: Vec<f64> = self.history.borrow().samples.iter().filter_map(|s| s.watts).collect();
        match self.tab.get() {
            Tab::Cpu => (format!("CPU LOAD, {span}"), format!("Now {:.0}%  ·  {} cores", snapshot.cpu.total, snapshot.cpu.cores.len())),
            Tab::Memory => (format!("MEMORY USED, {span}"), format!("{} of {}", bytes(snapshot.memory.used), bytes(snapshot.memory.total))),
            Tab::Energy => (
                format!("POWER DRAW, {span}"),
                match &snapshot.power {
                    Some(power) => format!("Now {:.1} W  ·  average {:.1} W", power.watts, watts.iter().sum::<f64>() / watts.len().max(1) as f64),
                    None => "Not measurable on AC without the power helper".to_owned(),
                },
            ),
            Tab::Disk => (format!("READ AND WRITTEN, {span}"), format!("Read {}  ·  written {}", rate(snapshot.disk.read as f64), rate(snapshot.disk.write as f64))),
            Tab::Network => (format!("RECEIVED AND SENT, {span}"), format!("In {}  ·  out {}", rate(snapshot.network.read as f64), rate(snapshot.network.write as f64))),
        }
    }

    /// The summary cards for the current tab: title, big figure, lines, and a meter (0 to 100).
    fn cards(&self, s: &Snapshot) -> Vec<(String, String, Vec<String>, Option<f64>)> {
        let busiest = s.apps.iter().max_by(|a, b| a.cpu.total_cmp(&b.cpu));
        let history = self.history.borrow();
        match self.tab.get() {
            Tab::Cpu => {
                let busiest_core = s.cpu.cores.iter().copied().fold(0.0, f64::max);
                let mut cards = Vec::new();
                if let Some(app) = busiest {
                    let mut lines = vec![format!("{:.0}% of a core", app.cpu)];
                    lines.extend(history.flag(&app.id));
                    cards.push(("BUSIEST APP".to_owned(), app.name.clone(), lines, None));
                }
                cards.push(("CORES".to_owned(), format!("{:.0}%", s.cpu.total), vec![format!("{} cores", s.cpu.cores.len()), format!("Busiest core {busiest_core:.0}%")], Some(s.cpu.total)));
                cards
            }
            Tab::Memory => {
                let top = s.apps.iter().max_by_key(|app| app.memory);
                let used = s.memory.used as f64 * 100.0 / s.memory.total.max(1) as f64;
                let swap = s.memory.swap_used as f64 * 100.0 / s.memory.swap_total.max(1) as f64;
                vec![
                    ("IN USE".to_owned(), bytes(s.memory.used), vec![format!("{} available", bytes(s.memory.available)), top.map_or(String::new(), |app| format!("{} uses the most", app.name))], Some(used)),
                    ("SWAP".to_owned(), bytes(s.memory.swap_used), vec![format!("of {}", bytes(s.memory.swap_total))], Some(swap)),
                ]
            }
            Tab::Energy => {
                let battery = match &s.battery {
                    Some(b) => {
                        let mut lines = vec![match b.watts {
                            Some(watts) => format!("{}  ·  {watts:.1} W", b.status),
                            None => b.status.clone(),
                        }];
                        let health = [b.health.map(|h| format!("Health {h:.0}%")), b.cycles.map(|c| format!("{c} cycles"))].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
                        if !health.is_empty() {
                            lines.push(health);
                        }
                        ("BATTERY".to_owned(), format!("{:.0}%", b.percent), lines, Some(b.percent))
                    }
                    None => ("BATTERY".to_owned(), "None".to_owned(), vec!["Running on AC power".to_owned()], None),
                };
                let significant: Vec<&App> = s
                    .apps
                    .iter()
                    .filter(|app| match (&s.power, app.watts) {
                        (Some(power), Some(watts)) => watts / power.watts.max(1e-9) > 0.2,
                        _ => app.cpu > 50.0,
                    })
                    .collect();
                let mut lines = Vec::new();
                if let Some(app) = significant.first() {
                    match (&s.power, app.watts) {
                        (Some(power), Some(watts)) => {
                            lines.push(format!("{}: {:.0}% of the power drawn", app.name, watts / power.watts.max(1e-9) * 100.0));
                            lines.push(format!("Quitting it saves about {watts:.1} W"));
                        }
                        _ => lines.push(format!("{}: {:.0}% of a core", app.name, app.cpu)),
                    }
                } else {
                    lines.push("Nothing is draining the battery".to_owned());
                }
                let count = significant.len();
                let big = match count {
                    0 => "None".to_owned(),
                    1 => "1 app".to_owned(),
                    n => format!("{n} apps"),
                };
                vec![battery, ("SIGNIFICANT ENERGY".to_owned(), big, lines, None)]
            }
            Tab::Disk => {
                let top = s.apps.iter().max_by_key(|app| app.disk.read + app.disk.write);
                let mut cards = vec![("NOW".to_owned(), rate((s.disk.read + s.disk.write) as f64), vec![format!("Read {}", rate(s.disk.read as f64)), format!("Written {}", rate(s.disk.write as f64))], None)];
                if let Some(app) = top.filter(|app| app.disk.read + app.disk.write > 0) {
                    cards.push(("BUSIEST APP".to_owned(), app.name.clone(), vec![format!("Reading {}", rate(app.disk.read as f64)), format!("Writing {}", rate(app.disk.write as f64))], None));
                }
                cards
            }
            Tab::Network => vec![("NOW".to_owned(), rate((s.network.read + s.network.write) as f64), vec![format!("Received {}", rate(s.network.read as f64)), format!("Sent {}", rate(s.network.write as f64))], None)],
        }
    }
}

fn cell(text: String, width: i32) -> Measure {
    Measure { text, width, bar: None }
}

fn append_cells(row: &gtk::Box, cells: Vec<Measure>) {
    for cell in cells {
        let holder = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        holder.set_size_request(cell.width, -1);
        // Columns keep their width; only the name stretches.
        holder.set_hexpand(false);
        if let Some((value, warm)) = cell.bar {
            let bar = meter((value * 1.4).min(100.0), warm);
            bar.set_size_request(70, 6);
            bar.set_valign(gtk::Align::Center);
            holder.append(&bar);
        }
        let label = gtk::Label::builder().label(&cell.text).xalign(1.0).hexpand(true).build();
        label.add_css_class("number");
        holder.append(&label);
        row.append(&holder);
    }
}

fn process_row(name: &str, pid: u32, cells: Vec<Measure>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("row");
    row.add_css_class("process");
    let label = gtk::Label::builder().label(name).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).tooltip_text(format!("pid {pid}")).build();
    label.add_css_class("name");
    row.append(&label);
    append_cells(&row, cells);
    row
}

fn card(title: &str, big: &str, lines: &[String], meter: Option<f64>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("card");
    let title = gtk::Label::builder().label(title).xalign(0.0).build();
    title.add_css_class("card-title");
    let big = gtk::Label::builder().label(big).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
    big.add_css_class("big");
    card.append(&title);
    card.append(&big);
    for line in lines {
        let label = gtk::Label::builder().label(line).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
        label.add_css_class("line");
        card.append(&label);
    }
    if let Some(value) = meter {
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        card.append(&spacer);
        let bar = self::meter(value, false);
        bar.set_size_request(-1, 6);
        card.append(&bar);
    }
    card
}

/// A rounded bar filled `value` percent, sage or, when `warm`, amber.
fn meter(value: f64, warm: bool) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_draw_func(move |_, cr, width, height| {
        let (width, height) = (width as f64, height as f64);
        let rounded = |w: f64| {
            let r = (height / 2.0).min(w / 2.0);
            cr.new_sub_path();
            cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            cr.arc(r, r, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.close_path();
        };
        rounded(width);
        let palette = &theme::look().palette;
        theme::set_source(cr, &palette.text, 0.08);
        let _ = cr.fill();
        let filled = width * value.clamp(0.0, 100.0) / 100.0;
        if filled >= 1.0 {
            rounded(filled.max(height));
            theme::set_source(cr, if warm { &palette.warning } else { &palette.accent }, 1.0);
            let _ = cr.fill();
        }
    });
    area
}

/// The app's own icon from its desktop file, or a tile with its first letter.
pub fn app_icon(app: &App, size: i32) -> gtk::Widget {
    if let Some(icon) = app.desktop_id.as_deref().and_then(desktop_icon) {
        let image = gtk::Image::from_gicon(&icon);
        image.set_pixel_size(size);
        image.set_valign(gtk::Align::Center);
        return image.upcast();
    }
    let letter = app.name.chars().next().map(|c| c.to_string()).unwrap_or_default();
    let tile = gtk::Label::new(Some(&letter));
    tile.add_css_class("tile");
    tile.set_size_request(size, size);
    tile.set_valign(gtk::Align::Center);
    tile.upcast()
}

/// A desktop file's icon, looked up once per app: rows are rebuilt every second.
fn desktop_icon(desktop_id: &str) -> Option<gio::Icon> {
    thread_local! {
        static ICONS: RefCell<std::collections::HashMap<String, Option<gio::Icon>>> = RefCell::default();
    }
    ICONS.with_borrow_mut(|icons| {
        icons
            .entry(desktop_id.to_owned())
            .or_insert_with(|| gio::AppInfo::all().into_iter().find(|info| info.id().as_deref() == Some(desktop_id)).and_then(|info| info.icon()))
            .clone()
    })
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 { format!("{:.1} GB", n / 1e9) } else { format!("{:.0} MB", n / 1e6) }
}

fn rate(n: f64) -> String {
    if n >= 1e6 { format!("{:.1} MB/s", n / 1e6) } else { format!("{:.0} KB/s", n / 1e3) }
}
