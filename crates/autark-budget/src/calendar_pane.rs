//! Calendar: a month with the paydays, what falls due each day and what you spent. Pick a day to see it in full.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_finance::calendar::{self, Day, Month};
use autark_finance::store::Store;
use chrono::{Datelike, NaiveDate};
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;

use crate::settings::Settings;
use crate::text;
use crate::ui::{clear, dim, line, row, rows, section, small_button};
use crate::view;

/// Only this many bills are named in a day's box; the rest are counted.
const CHIPS: usize = 2;

pub struct CalendarPane {
    pub root: gtk::ScrolledWindow,
    body: gtk::Box,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    /// The month shown, as (year, month).
    shown: Cell<(i32, u32)>,
    selected: Cell<Option<NaiveDate>>,
}

impl CalendarPane {
    pub fn new(store: Store, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let (root, body) = crate::ui::page();
        let today = view::today();
        let pane = Rc::new(Self { root, body, store, settings, shown: Cell::new((today.year(), today.month())), selected: Cell::new(Some(today)) });
        pane.refresh();
        pane
    }

    /// Opened again, it shows this month with today picked, like a calendar on the wall.
    pub fn show_today(self: &Rc<Self>) {
        let today = view::today();
        self.shown.set((today.year(), today.month()));
        self.selected.set(Some(today));
        self.refresh();
    }

    fn go(self: &Rc<Self>, delta: i32) {
        let (year, month) = self.shown.get();
        self.shown.set(calendar::shift(year, month, delta));
        self.selected.set(None);
        self.refresh();
    }

    pub fn refresh(self: &Rc<Self>) {
        clear(&self.body);
        let (year, month_number) = self.shown.get();
        let today = view::today();
        let settings = self.settings.borrow();
        let Some(month) = view::month(&self.store, &settings, year, month_number, today) else { return };

        self.body.append(&self.header(&month));
        self.body.append(&dim(&format!("{} {}  ·  {} due  ·  {} spent", month.paydays, if month.paydays == 1 { "payday" } else { "paydays" }, month.due, month.spent)));
        self.body.append(&self.grid(&month));
        if let Some(day) = self.selected.get().and_then(|date| month.days().find(|d| d.date == date)) {
            self.body.append(&self.detail(day));
        } else {
            self.body.append(&dim("Pick a day to see what falls due and what you spent."));
        }
    }

    fn header(self: &Rc<Self>, month: &Month) -> gtk::Box {
        let title = chrono::NaiveDate::from_ymd_opt(month.year, month.month, 1).map_or(String::new(), |d| d.format("%B %Y").to_string());
        let title = gtk::Label::builder().label(title).xalign(0.0).hexpand(true).build();
        title.add_css_class("projected");
        let (previous, next, today) = (small_button("‹"), small_button("›"), small_button("Today"));
        previous.set_tooltip_text(Some("Last month"));
        next.set_tooltip_text(Some("Next month"));
        for (button, delta) in [(&previous, -1), (&next, 1)] {
            let me = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(me) = me.upgrade() else { return };
                // Rebuilding inside the click would destroy the button that is handling it.
                glib::idle_add_local_once(move || me.go(delta));
            });
        }
        let me = Rc::downgrade(self);
        today.connect_clicked(move |_| {
            let Some(me) = me.upgrade() else { return };
            glib::idle_add_local_once(move || me.show_today());
        });
        let bar = line(&[title.upcast_ref(), today.upcast_ref(), previous.upcast_ref(), next.upcast_ref()]);
        bar.set_margin_top(8);
        bar
    }

    fn grid(self: &Rc<Self>, month: &Month) -> gtk::Grid {
        let grid = gtk::Grid::builder().column_homogeneous(true).column_spacing(4).row_spacing(4).margin_top(10).build();
        for (column, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
            let label = gtk::Label::builder().label(*name).build();
            label.add_css_class("dim");
            grid.attach(&label, column as i32, 0, 1, 1);
        }
        for (week, days) in month.weeks.iter().enumerate() {
            for (column, day) in days.iter().enumerate() {
                grid.attach(&self.cell(day), column as i32, week as i32 + 1, 1, 1);
            }
        }
        grid
    }

    fn cell(self: &Rc<Self>, day: &Day) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("cal-day");
        for (class, on) in [("out", !day.in_month), ("today", day.today), ("payday", day.payday), ("selected", self.selected.get() == Some(day.date))] {
            if on {
                button.add_css_class(class);
            }
        }
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let number = gtk::Label::builder().label(day.date.day().to_string()).xalign(0.0).hexpand(true).build();
        number.add_css_class("cal-num");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        top.append(&number);
        if day.payday {
            let tag = gtk::Label::builder().label("payday").build();
            tag.add_css_class("cal-payday");
            top.append(&tag);
        }
        inner.append(&top);
        for line in day.lines.iter().take(CHIPS) {
            // The name gives way when the box is narrow; the amount never does.
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let name = gtk::Label::builder().label(format!("{}{}", if line.paid { "✓ " } else { "" }, line.name)).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            let amount = gtk::Label::builder().label(format!("{}{}", text::confidence_mark(line.confidence), line.amount)).xalign(1.0).build();
            for label in [&name, &amount] {
                label.add_css_class("cal-chip");
                if line.paid {
                    label.add_css_class("paid");
                }
            }
            chip.append(&name);
            chip.append(&amount);
            inner.append(&chip);
        }
        if day.lines.len() > CHIPS {
            let more = gtk::Label::builder().label(format!("+{} more", day.lines.len() - CHIPS)).xalign(0.0).build();
            more.add_css_class("cal-chip");
            inner.append(&more);
        }
        if day.purchases > 0 {
            let spent = gtk::Label::builder().label(format!("spent {}", day.spent)).xalign(0.0).ellipsize(pango::EllipsizeMode::End).build();
            spent.add_css_class("cal-spent");
            inner.append(&spent);
        }
        button.set_child(Some(&inner));

        let (me, date) = (Rc::downgrade(self), day.date);
        button.connect_clicked(move |_| {
            let Some(me) = me.upgrade() else { return };
            me.selected.set(Some(date));
            glib::idle_add_local_once(move || me.refresh());
        });
        button
    }

    fn detail(&self, day: &Day) -> gtk::Box {
        let detail = section(&day.date.format("%A, %-d %B").to_string());
        let list = rows();
        if day.payday {
            list.append(&row("Payday", &self.settings.borrow().finance.paycheck.to_string()));
        }
        for line in &day.lines {
            let status = if line.paid { "  ✓ paid" } else { "" };
            list.append(&row(&format!("{}{status}", line.name), &format!("{}{}", text::confidence_mark(line.confidence), line.amount)));
        }
        let mut spent: Vec<_> = self.store.spending().into_iter().filter(|s| s.date == day.date).collect();
        spent.reverse();
        for spend in spent {
            let what = if spend.category.is_empty() { "Other".to_string() } else { spend.category.clone() };
            let label = if spend.note.is_empty() { format!("Spent: {what}") } else { format!("Spent: {what} · {}", spend.note) };
            list.append(&row(&label, &spend.amount.to_string()));
        }
        if list.first_child().is_none() {
            detail.append(&dim("Nothing on this day."));
        } else {
            detail.append(&list);
            if day.due().centavos() > 0 {
                detail.append(&dim(&format!("{} falls due.", day.due())));
            }
        }
        detail
    }
}
