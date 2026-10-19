//! The calendar's window: a month grid, and beside it the selected day's events. Double-clicking a day, or the
//! New button, opens the editor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use autark_common::theme::rgb;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime};
use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::backend::Backend;
use crate::{editor, logic};

pub struct Calendar {
    pub window: gtk::ApplicationWindow,
    pub backend: Arc<Backend>,
    month: Cell<(i32, u32)>,
    selected: Cell<NaiveDate>,
    week_start: chrono::Weekday,
    title: gtk::Label,
    grid: gtk::Grid,
    agenda: gtk::Box,
    status: gtk::Label,
    /// Whether a fetch is under way, so a second isn't started on top of it.
    syncing: Cell<bool>,
    me: RefCell<std::rc::Weak<Calendar>>,
}

fn dot(color: &str) -> gtk::DrawingArea {
    let [r, g, b] = rgb(color);
    let area = gtk::DrawingArea::builder().content_width(8).content_height(8).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgb(r, g, b);
        cr.arc(f64::from(width) / 2.0, f64::from(height) / 2.0, f64::from(width.min(height)) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    area
}

fn clear(container: &impl IsA<gtk::Widget>) {
    while let Some(child) = container.first_child() {
        child.unparent();
    }
}

impl Calendar {
    pub fn new(app: &gtk::Application, backend: Arc<Backend>) -> Rc<Self> {
        let config: crate::Config = autark_common::config::load("calendar");
        let today = Local::now().date_naive();
        let window = gtk::ApplicationWindow::builder().application(app).title("Calendar").default_width(1100).default_height(760).build();
        window.add_css_class("calendar");

        let title = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        title.add_css_class("month");
        let nav = |label: &str| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("nav");
            button
        };
        let (previous, next, now) = (nav("‹"), nav("›"), gtk::Button::with_label("Today"));
        now.add_css_class("nav");
        let new = gtk::Button::with_label("New event");
        new.add_css_class("primary");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("titlebar-row");
        for widget in [title.upcast_ref::<gtk::Widget>(), previous.upcast_ref(), now.upcast_ref(), next.upcast_ref(), new.upcast_ref()] {
            bar.append(widget);
        }
        let header = gtk::HeaderBar::new();
        header.set_title_widget(Some(&gtk::Label::new(Some("Calendar"))));
        window.set_titlebar(Some(&header));

        let grid = gtk::Grid::builder().column_homogeneous(true).row_homogeneous(false).hexpand(true).vexpand(true).build();
        let agenda = gtk::Box::new(gtk::Orientation::Vertical, 8);
        agenda.add_css_class("agenda");
        agenda.set_size_request(320, -1);
        let status = gtk::Label::builder().xalign(0.0).build();
        status.add_css_class("status");

        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.set_hexpand(true);
        left.append(&bar);
        left.append(&grid);
        left.append(&status);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&left);
        root.append(&agenda);
        window.set_child(Some(&root));

        let calendar = Rc::new(Self {
            window,
            backend,
            month: Cell::new((today.year(), today.month())),
            selected: Cell::new(today),
            week_start: logic::week_start(&config.week_start),
            title,
            grid,
            agenda,
            status,
            syncing: Cell::new(false),
            me: RefCell::default(),
        });
        *calendar.me.borrow_mut() = Rc::downgrade(&calendar);

        for (button, delta) in [(&previous, -1), (&next, 1)] {
            let me = Rc::downgrade(&calendar);
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    let (year, month) = logic::shift_month(me.month.get().0, me.month.get().1, delta);
                    me.go(year, month);
                }
            });
        }
        let me = Rc::downgrade(&calendar);
        now.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                let today = Local::now().date_naive();
                me.selected.set(today);
                me.go(today.year(), today.month());
            }
        });
        let me = Rc::downgrade(&calendar);
        new.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.add(me.selected.get());
            }
        });

        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(&calendar);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(me) = me.upgrade() else { return glib::Propagation::Proceed };
            let shift = match key {
                gdk::Key::Page_Up => -1,
                gdk::Key::Page_Down => 1,
                gdk::Key::n => {
                    me.add(me.selected.get());
                    return glib::Propagation::Stop;
                }
                gdk::Key::t => {
                    let today = Local::now().date_naive();
                    me.selected.set(today);
                    me.go(today.year(), today.month());
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            let (year, month) = logic::shift_month(me.month.get().0, me.month.get().1, shift);
            me.go(year, month);
            glib::Propagation::Stop
        });
        calendar.window.add_controller(keys);

        calendar.draw();
        calendar.sync();
        calendar
    }

    fn go(self: &Rc<Self>, year: i32, month: u32) {
        self.month.set((year, month));
        self.draw();
        self.sync();
    }

    /// The days the grid shows, which are what to fetch.
    fn range(&self) -> (NaiveDate, NaiveDate) {
        let cells = logic::month_cells(self.month.get().0, self.month.get().1, self.week_start);
        (cells[0], cells[41] + Duration::days(1))
    }

    /// Fetches the shown days from every account, then redraws.
    pub fn sync(self: &Rc<Self>) {
        if self.syncing.replace(true) {
            return;
        }
        let (from, to) = self.range();
        let (backend, me) = (self.backend.clone(), Rc::downgrade(self));
        self.status.remove_css_class("error");
        self.status.set_text("Syncing…");
        glib::spawn_future_local(async move {
            let problems = blocking::unblock(move || backend.refresh(from, to)).await;
            let Some(me) = me.upgrade() else { return };
            me.syncing.set(false);
            if problems.is_empty() {
                let count = me.backend.snapshot().len();
                me.status.set_text(&if count == 0 { "No calendars yet. Add an account: autark-accounts add-google".to_owned() } else { format!("{count} calendar{}", if count == 1 { "" } else { "s" }) });
            } else {
                me.status.add_css_class("error");
                me.status.set_text(&problems.join("  ·  "));
            }
            // The shown month may have changed while this was fetching.
            if me.range() != (from, to) {
                me.sync();
            }
            me.draw();
        });
    }

    fn select(self: &Rc<Self>, day: NaiveDate) {
        self.selected.set(day);
        if (day.year(), day.month()) != self.month.get() {
            self.go(day.year(), day.month());
        } else {
            self.draw();
        }
    }

    /// The showings on `day`, from every calendar.
    fn day(&self, day: NaiveDate) -> Vec<logic::Showing> {
        let mut all: Vec<logic::Showing> = self
            .backend
            .snapshot()
            .iter()
            .flat_map(|cal| cal.resources.iter().flat_map(move |stored| logic::showings(&stored.parsed, &stored.url, &cal.name, &cal.color, day)))
            .collect();
        all.sort_by(|a, b| b.all_day.cmp(&a.all_day).then(a.start.cmp(&b.start)).then(a.summary.cmp(&b.summary)));
        all
    }

    pub fn draw(self: &Rc<Self>) {
        let (year, month) = self.month.get();
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
        self.title.set_text(&first.format("%B %Y").to_string());
        clear(&self.grid);
        let today = Local::now().date_naive();
        let cells = logic::month_cells(year, month, self.week_start);
        for column in 0..7 {
            let label = gtk::Label::builder().label(cells[column].format("%a").to_string()).xalign(0.0).build();
            label.add_css_class("weekday");
            self.grid.attach(&label, column as i32, 0, 1, 1);
        }
        for (n, day) in cells.iter().copied().enumerate() {
            let showings = self.day(day);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let number = gtk::Label::builder().label(day.day().to_string()).xalign(0.0).halign(gtk::Align::Start).build();
            number.add_css_class("number");
            content.append(&number);
            for showing in showings.iter().take(3) {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
                row.append(&dot(&showing.color));
                let text = if showing.all_day { showing.summary.clone() } else { format!("{} {}", showing.start.format("%H:%M"), showing.summary) };
                let label = gtk::Label::builder().label(text).xalign(0.0).ellipsize(pango::EllipsizeMode::End).hexpand(true).build();
                label.add_css_class("chip");
                row.append(&label);
                content.append(&row);
            }
            if showings.len() > 3 {
                let more = gtk::Label::builder().label(format!("{} more", showings.len() - 3)).xalign(0.0).build();
                more.add_css_class("more");
                content.append(&more);
            }
            let button = gtk::Button::builder().child(&content).hexpand(true).vexpand(true).build();
            button.add_css_class("day");
            button.set_focus_on_click(false);
            if day.month() != month {
                button.add_css_class("other");
            }
            if day == today {
                button.add_css_class("today");
            }
            if day == self.selected.get() {
                button.add_css_class("selected");
            }
            let me = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.select(day);
                }
            });
            let click = gtk::GestureClick::new();
            let me = Rc::downgrade(self);
            click.connect_pressed(move |_, presses, _, _| {
                if presses == 2
                    && let Some(me) = me.upgrade()
                {
                    me.add(day);
                }
            });
            button.add_controller(click);
            self.grid.attach(&button, (n % 7) as i32, (n / 7) as i32 + 1, 1, 1);
        }
        self.draw_agenda();
    }

    fn draw_agenda(self: &Rc<Self>) {
        clear(&self.agenda);
        let day = self.selected.get();
        let date = gtk::Label::builder().label(day.format("%A %-d %B").to_string()).xalign(0.0).build();
        date.add_css_class("date");
        self.agenda.append(&date);
        let showings = self.day(day);
        if showings.is_empty() {
            let empty = gtk::Label::builder().label("Nothing planned.").xalign(0.0).build();
            empty.add_css_class("empty");
            self.agenda.append(&empty);
        }
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("agenda-list");
        for showing in showings {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("event");
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.append(&dot(&showing.color));
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let name = gtk::Label::builder().label(&showing.summary).xalign(0.0).wrap(true).build();
            let when = if showing.all_day { "All day".to_owned() } else { format!("{} – {}", showing.start.format("%H:%M"), showing.end.format("%H:%M")) };
            let detail = if showing.location.is_empty() { format!("{when}  ·  {}", showing.calendar) } else { format!("{when}  ·  {}", showing.location) };
            let when = gtk::Label::builder().label(detail).xalign(0.0).build();
            when.add_css_class("when");
            text.append(&name);
            text.append(&when);
            line.append(&text);
            row.set_child(Some(&line));
            let click = gtk::GestureClick::new();
            let (me, showing) = (Rc::downgrade(self), showing.clone());
            click.connect_released(move |_, _, _, _| {
                if let Some(me) = me.upgrade() {
                    me.edit(&showing);
                }
            });
            row.add_controller(click);
            list.append(&row);
        }
        self.agenda.append(&list);
    }

    fn add(self: &Rc<Self>, day: NaiveDate) {
        editor::open(self, None, day);
    }

    fn edit(self: &Rc<Self>, showing: &logic::Showing) {
        editor::open(self, Some(showing.clone()), showing.start.date());
    }

    /// Called by the editor when it has changed something on a server.
    pub fn changed(self: &Rc<Self>, day: NaiveDate) {
        self.selected.set(day);
        self.go(day.year(), day.month());
    }
}

pub fn default_times() -> (NaiveTime, NaiveTime) {
    let now = Local::now().time();
    let start = NaiveTime::from_hms_opt(now.format("%H").to_string().parse::<u32>().map_or(9, |hour| (hour + 1).min(22)), 0, 0).unwrap_or(NaiveTime::MIN);
    (start, start + Duration::hours(1))
}
