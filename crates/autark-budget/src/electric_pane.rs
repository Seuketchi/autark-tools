//! The electricity pane, in five tabs: Overview (the projected bill, tips, what-if), Trends (days, weeks, billing
//! cycles and weekdays), Log (a reading, and the whole history by month), Bills, and Settings (budget, cycle and the
//! thresholds the tips use). Every number comes from `autark_finance`, through `text`, the same as the terminal shows.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use autark_finance::electric::{self, Basis, Bill, Reading};
use autark_finance::money::Money;
use autark_finance::recommend::{self, Level, Tip};
use autark_finance::store::Store;
use autark_finance::trends;
use chrono::{Datelike, Local, NaiveDate};
use gtk::prelude::*;

use crate::bars::{Bars, Data};
use crate::cli::{check_reading_in, parse_date, parse_number};
use crate::mascot::{self, Mascot};
use crate::settings::Settings;
use crate::text;
use crate::ui::{clear, dim, entry, line, note, page, row, rows, section, small_button};

/// Months of history shown at first, and added each time "Show older" is pressed.
const MONTHS: usize = 3;

/// A reading in the history: its place in the file (to edit or delete it), the reading, and its change from the one before.
type Entry = (usize, Reading, Option<f64>);

/// A caption's two ends under a chart: where it starts and where it stops.
struct Ends {
    row: gtk::Box,
    first: gtk::Label,
    last: gtk::Label,
}

impl Ends {
    fn new() -> Ends {
        let first = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        let last = gtk::Label::builder().xalign(1.0).build();
        let row = line(&[first.upcast_ref(), last.upcast_ref()]);
        row.add_css_class("ends");
        Ends { row, first, last }
    }

    fn set(&self, first: &str, last: &str) {
        self.first.set_label(first);
        self.last.set_label(last);
    }
}

pub struct ElectricPane {
    pub root: gtk::Box,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    stack: gtk::Stack,

    // Overview.
    projected: gtk::Label,
    verdict: gtk::Label,
    basis: gtk::Label,
    facts: gtk::Label,
    mascot: Mascot,
    say: gtk::Label,
    tips: gtk::Box,
    watts: gtk::Entry,
    hours: gtk::Entry,
    what_if: gtk::Label,

    // Trends.
    daily: Bars,
    daily_ends: Ends,
    weekly: Bars,
    weekly_ends: Ends,
    weekly_text: gtk::Label,
    cycles: Bars,
    cycle_text: gtk::Label,
    weekday: Bars,
    trend_text: gtk::Label,

    // Log.
    kwh: gtk::Entry,
    date: gtk::Entry,
    add: gtk::Button,
    restore: gtk::Button,
    log_note: gtk::Label,
    /// A reading that looked wrong, held until it is added a second time on purpose.
    held: Cell<Option<(NaiveDate, f64)>>,
    deleted: RefCell<Option<Reading>>,
    history: gtk::Box,
    older: gtk::Button,
    months_shown: Cell<usize>,
    /// Older months the user opened, so they stay open when the list is redrawn.
    opened: RefCell<HashSet<(i32, u32)>>,

    // Bills.
    bill_kwh: gtk::Entry,
    bill_amount: gtk::Entry,
    bill_from: gtk::Entry,
    bill_to: gtk::Entry,
    bill_note: gtk::Label,
    bills: gtk::Box,

    // Settings.
    budget: gtk::Entry,
    billing_day: gtk::Entry,
    rate: gtk::Entry,
    near: gtk::Entry,
    comfortable: gtk::Entry,
    week: gtk::Entry,
    gap: gtk::Entry,
    over: gtk::Entry,
    settings_note: gtk::Label,
}

fn tip_card(tip: &Tip) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 2);
    card.add_css_class("tip");
    card.add_css_class(match tip.level {
        Level::Alert => "alert",
        Level::Warn => "warn",
        Level::Info => "info",
        Level::Good => "good",
    });
    let title = gtk::Label::builder().label(&tip.title).xalign(0.0).wrap(true).build();
    title.add_css_class("tip-title");
    let detail = gtk::Label::builder().label(&tip.detail).xalign(0.0).wrap(true).build();
    detail.add_css_class("dim");
    card.append(&title);
    card.append(&detail);
    card
}

fn percent(fraction: f64) -> String {
    format!("{}", (fraction * 1000.0).round() / 10.0)
}

impl ElectricPane {
    pub fn new(store: Store, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let label = |class: &str| {
            let label = gtk::Label::builder().xalign(0.0).build();
            label.add_css_class(class);
            label
        };
        let wrapped = || gtk::Label::builder().xalign(0.0).wrap(true).build();
        let pane = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 0),
            store,
            settings,
            stack: gtk::Stack::builder().hexpand(true).vexpand(true).build(),
            projected: label("projected"),
            verdict: label("verdict"),
            basis: {
                let basis = wrapped();
                basis.add_css_class("dim");
                basis
            },
            facts: wrapped(),
            mascot: Mascot::new(),
            say: gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).max_width_chars(18).build(),
            tips: gtk::Box::new(gtk::Orientation::Vertical, 8),
            watts: entry("Watts", 8),
            hours: entry("Hours a day", 11),
            what_if: note(),
            daily: Bars::new(96),
            daily_ends: Ends::new(),
            weekly: Bars::new(80),
            weekly_ends: Ends::new(),
            weekly_text: wrapped(),
            cycles: Bars::new(80),
            cycle_text: wrapped(),
            weekday: Bars::new(56),
            trend_text: wrapped(),
            kwh: entry("Meter reading, kWh", 18),
            date: entry("Date (today)", 12),
            add: gtk::Button::with_label("Add"),
            restore: gtk::Button::with_label("Restore"),
            log_note: note(),
            held: Cell::new(None),
            deleted: RefCell::new(None),
            history: gtk::Box::new(gtk::Orientation::Vertical, 0),
            older: gtk::Button::with_label("Show older months"),
            months_shown: Cell::new(MONTHS),
            opened: RefCell::new(HashSet::new()),
            bill_kwh: entry("kWh on the bill", 14),
            bill_amount: entry("Amount, ₱", 12),
            bill_from: entry("From YYYY-MM-DD", 14),
            bill_to: entry("To YYYY-MM-DD", 14),
            bill_note: note(),
            bills: rows(),
            budget: entry("Budget, ₱", 10),
            billing_day: entry("Day 1-28", 9),
            rate: entry("₱ per kWh", 10),
            near: entry("%", 6),
            comfortable: entry("%", 6),
            week: entry("%", 6),
            gap: entry("%", 6),
            over: entry("%", 6),
            settings_note: note(),
        });
        pane.build();
        pane.show_settings();
        pane.refresh();
        pane
    }

    fn build(self: &Rc<Self>) {
        self.stack.set_widget_name("electric-tabs");
        self.stack.add_named(&self.overview_page(), Some("overview"));
        self.stack.add_named(&self.trends_page(), Some("trends"));
        self.stack.add_named(&self.log_page(), Some("log"));
        self.stack.add_named(&self.bills_page(), Some("bills"));
        self.stack.add_named(&self.settings_page(), Some("settings"));

        // The tab bar: a row of toggle buttons, one group, each showing its page.
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs.add_css_class("tabs");
        let mut first: Option<gtk::ToggleButton> = None;
        for (name, title) in [("overview", "Overview"), ("trends", "Trends"), ("log", "Log"), ("bills", "Bills"), ("settings", "Settings")] {
            let tab = gtk::ToggleButton::with_label(title);
            tab.add_css_class("tab");
            match &first {
                Some(group) => tab.set_group(Some(group)),
                None => {
                    tab.set_active(true);
                    first = Some(tab.clone());
                }
            }
            let stack = self.stack.clone();
            tab.connect_toggled(move |tab| {
                if tab.is_active() {
                    stack.set_visible_child_name(name);
                }
            });
            tabs.append(&tab);
        }
        self.root.append(&tabs);
        self.root.append(&self.stack);

        // Entries answer Enter, buttons answer a click, and each does its job on the pane.
        let connect = |entry: &gtk::Entry, action: fn(&Rc<ElectricPane>)| {
            let me = Rc::downgrade(self);
            entry.connect_activate(move |_| {
                if let Some(me) = me.upgrade() {
                    action(&me);
                }
            });
        };
        let click = |button: &gtk::Button, action: fn(&Rc<ElectricPane>)| {
            let me = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    action(&me);
                }
            });
        };
        for field in [&self.kwh, &self.date] {
            connect(field, ElectricPane::add_reading);
        }
        click(&self.add, ElectricPane::add_reading);
        click(&self.restore, ElectricPane::restore_reading);
        click(&self.older, ElectricPane::show_older);
        for field in [&self.bill_kwh, &self.bill_amount, &self.bill_from, &self.bill_to] {
            connect(field, ElectricPane::record_bill);
        }
        for field in [&self.budget, &self.billing_day, &self.rate, &self.near, &self.comfortable, &self.week, &self.gap, &self.over] {
            connect(field, ElectricPane::save_settings);
        }
        for field in [&self.watts, &self.hours] {
            let me = Rc::downgrade(self);
            field.connect_changed(move |_| {
                if let Some(me) = me.upgrade() {
                    me.show_what_if();
                }
            });
        }
    }

    fn overview_page(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let (scrolled, page) = page();

        // The projection, with the mascot beside it.
        let summary = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        summary.add_css_class("summary");
        let words = gtk::Box::new(gtk::Orientation::Vertical, 4);
        words.set_hexpand(true);
        for widget in [dim("Projected bill this cycle").upcast::<gtk::Widget>(), self.projected.clone().upcast(), self.verdict.clone().upcast(), self.basis.clone().upcast(), self.facts.clone().upcast()] {
            words.append(&widget);
        }
        let buddy = gtk::Box::new(gtk::Orientation::Vertical, 8);
        buddy.set_valign(gtk::Align::Center);
        self.say.add_css_class("say");
        buddy.append(&self.mascot.area);
        buddy.append(&self.say);
        summary.append(&words);
        summary.append(&buddy);
        page.append(&summary);

        let tips = section("What to change");
        tips.append(&dim("Each tip speaks up only when a number crosses a threshold. You can move the thresholds in Settings."));
        tips.append(&self.tips);
        page.append(&tips);

        let what = section("What if I add…");
        what.append(&line(&[self.watts.upcast_ref(), self.hours.upcast_ref()]));
        what.append(&self.what_if);
        page.append(&what);
        scrolled
    }

    fn trends_page(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let (scrolled, page) = page();

        let days = section("Days");
        days.append(&dim("The last 14 days. The dashed line is what your budget allows a day; days over it are red, and paler bars are averages over days with no reading."));
        days.append(&self.daily.area);
        days.append(&self.daily_ends.row);
        page.append(&days);

        let weeks = section("Weeks");
        weeks.append(&dim("Average kWh a day, Monday to Sunday. Paler is a week still in progress or only partly logged."));
        weeks.append(&self.weekly.area);
        weeks.append(&self.weekly_ends.row);
        weeks.append(&self.weekly_text);
        page.append(&weeks);

        let months = section("Billing cycles");
        months.append(&dim("Average kWh a day for each cycle, and what it cost at your current rate."));
        months.append(&self.cycles.area);
        months.append(&self.cycle_text);
        page.append(&months);

        let weekday = section("By weekday");
        weekday.append(&dim("Average use on each day of the week, once there are about two weeks of daily readings."));
        weekday.append(&self.weekday.area);
        let names = gtk::Box::builder().homogeneous(true).build();
        for name in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
            names.append(&dim(name));
        }
        weekday.append(&names);
        weekday.append(&self.trend_text);
        page.append(&weekday);
        scrolled
    }

    fn log_page(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let (scrolled, page) = page();
        let log = section("Log a reading");
        self.add.add_css_class("suggested-action");
        self.restore.set_visible(false);
        let undo = gtk::Button::with_label("Undo last");
        let me = Rc::downgrade(self);
        undo.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.undo();
            }
        });
        log.append(&line(&[self.kwh.upcast_ref(), self.date.upcast_ref(), self.add.upcast_ref(), undo.upcast_ref()]));
        log.append(&line(&[self.log_note.upcast_ref(), self.restore.upcast_ref()]));
        page.append(&log);

        let history = section("History");
        history.append(&self.history);
        self.older.set_visible(false);
        history.append(&self.older);
        page.append(&history);
        scrolled
    }

    fn bills_page(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let (scrolled, page) = page();
        let record = section("Record a bill");
        record.append(&dim("When a bill arrives, the rate for the next projection comes from it."));
        let button = gtk::Button::with_label("Record bill");
        button.add_css_class("suggested-action");
        let me = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.record_bill();
            }
        });
        record.append(&line(&[self.bill_from.upcast_ref(), self.bill_to.upcast_ref()]));
        record.append(&line(&[self.bill_kwh.upcast_ref(), self.bill_amount.upcast_ref(), button.upcast_ref()]));
        record.append(&self.bill_note);
        page.append(&record);

        let bills = section("Past bills");
        bills.append(&self.bills);
        page.append(&bills);
        scrolled
    }

    fn settings_page(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let (scrolled, page) = page();
        let labelled = |text: &str, widget: &gtk::Entry, unit: &str| {
            let label = gtk::Label::builder().label(text).xalign(0.0).width_chars(34).build();
            label.add_css_class("dim");
            line(&[label.upcast_ref(), widget.upcast_ref(), dim(unit).upcast_ref()])
        };
        let basics = section("Budget");
        basics.append(&labelled("Budget for a cycle", &self.budget, "₱"));
        basics.append(&labelled("A cycle starts on day", &self.billing_day, "of the month, 1 to 28"));
        basics.append(&labelled("Rate until a bill gives yours", &self.rate, "₱ per kWh"));
        page.append(&basics);

        let thresholds = section("When tips speak up");
        thresholds.append(&dim("A tip appears only when its number crosses its line. Raise a line to hear less, lower it to hear sooner."));
        thresholds.append(&labelled("Close to the budget at", &self.near, "% of it or more"));
        thresholds.append(&labelled("Plenty of room at", &self.comfortable, "% of it or less"));
        thresholds.append(&labelled("A week up or down by", &self.week, "% on the week before"));
        thresholds.append(&labelled("A weekday busier than the quietest by", &self.gap, "%"));
        thresholds.append(&labelled("Too many days over, at", &self.over, "% of the last 14"));
        page.append(&thresholds);

        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        let me = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.save_settings();
            }
        });
        page.append(&line(&[save.upcast_ref()]));
        page.append(&self.settings_note);
        scrolled
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn plan(&self) -> electric::Plan {
        electric::plan(&self.settings.borrow().electric, &self.store.readings(), &self.store.bills(), Self::today())
    }

    /// Redraws everything from what is on disk.
    pub fn refresh(self: &Rc<Self>) {
        let readings = self.store.readings();
        let bills = self.store.bills();
        let plan = electric::plan(&self.settings.borrow().electric, &readings, &bills, Self::today());
        let trends = trends::trends(&readings, &plan);

        // Overview.
        let none = plan.basis == Basis::None;
        self.projected.set_label(&if none { "—".to_string() } else { plan.projected_cost.to_string() });
        self.verdict.set_label(&text::verdict(&plan));
        self.verdict.remove_css_class("over");
        self.verdict.remove_css_class("under");
        if !none {
            self.verdict.add_css_class(if plan.is_over() { "over" } else { "under" });
        }
        self.basis.set_label(&text::basis(&plan));
        let mut facts = format!("{} · {} days, {} left\nUsed so far: {}", text::cycle(&plan), plan.cycle.days(), plan.days_left, text::kwh(plan.used_kwh));
        if !none {
            facts.push_str(&format!("\nPace: {} a day\n{}", text::kwh(plan.daily_kwh), text::allowance(&plan)));
        }
        facts.push_str(&format!("\nRate: {}", text::rate(&plan)));
        self.facts.set_label(&facts);
        let mood = mascot::mood_for(&plan);
        self.mascot.set(mood);
        self.say.set_label(mascot::line(mood));

        clear(&self.tips);
        for tip in recommend::recommend(&plan, &trends, &self.settings.borrow().electric.thresholds) {
            self.tips.append(&tip_card(&tip));
        }
        self.show_what_if();

        // Trends.
        let limit = (trends.budget_kwh_per_day > 0.0).then_some(trends.budget_kwh_per_day);
        self.daily.set(Data { values: trends.days.iter().map(|d| Some(d.kwh)).collect(), solid: trends.days.iter().map(|d| d.measured).collect(), limit });
        self.daily_ends.set(&trends.days.first().map_or(String::new(), |d| text::day(d.date)), &trends.days.last().map_or(String::new(), |d| text::day(d.date)));
        self.weekly.set(Data { values: trends.weeks.iter().map(|w| Some(w.kwh_per_day)).collect(), solid: trends.weeks.iter().map(|w| w.full()).collect(), limit });
        self.weekly_ends.set(&trends.weeks.first().map_or(String::new(), |w| format!("Week of {}", text::day(w.start))), &trends.weeks.last().map_or(String::new(), |w| format!("Week of {}", text::day(w.start))));
        self.weekly_text.set_label(&text::period_lines(&trends.weeks[trends.weeks.len().saturating_sub(4)..], "Week of").join("\n"));
        self.cycles.set(Data { values: trends.cycles.iter().map(|c| Some(c.kwh_per_day)).collect(), solid: trends.cycles.iter().map(|c| c.full()).collect(), limit });
        self.cycle_text.set_label(&text::cycle_lines(&trends.cycles).join("\n"));
        self.weekday.set(Data { values: trends.weekday.to_vec(), solid: vec![true; 7], limit });
        self.trend_text.set_label(&text::trend_lines(&trends).join("\n"));

        self.refresh_history(&readings);
        self.refresh_bills(&bills);
    }

    /// The whole history, newest first, by month. The latest month is open; older ones are a line each that opens on
    /// a click, and are only drawn when opened, so a long history is never a long list of widgets.
    fn refresh_history(self: &Rc<Self>, all: &[Reading]) {
        clear(&self.history);
        let mut oldest_first: Vec<(usize, &Reading)> = all.iter().enumerate().collect();
        oldest_first.sort_by_key(|(index, reading)| (reading.date, *index));
        if oldest_first.is_empty() {
            self.history.append(&row("No readings yet. Start with what the meter says today.", ""));
            self.older.set_visible(false);
            return;
        }

        // Months, newest first, each with its readings newest first.
        let mut months: Vec<((i32, u32), Vec<Entry>)> = Vec::new();
        for at in (0..oldest_first.len()).rev() {
            let (index, reading) = oldest_first[at];
            let change = at.checked_sub(1).map(|before| reading.kwh - oldest_first[before].1.kwh);
            let month = (reading.date.year(), reading.date.month());
            match months.last_mut() {
                Some((last, entries)) if *last == month => entries.push((index, reading.clone(), change)),
                _ => months.push((month, vec![(index, reading.clone(), change)])),
            }
        }

        let shown = self.months_shown.get();
        for (position, ((year, month), entries)) in months.iter().take(shown).enumerate() {
            let name = NaiveDate::from_ymd_opt(*year, *month, 1).map_or(String::new(), |first| first.format("%B %Y").to_string());
            let used: f64 = entries.iter().filter_map(|(_, _, change)| *change).sum();
            let summary = format!("{name}   ·   {} {}   ·   {} used", entries.len(), if entries.len() == 1 { "reading" } else { "readings" }, text::kwh(used));
            if position == 0 {
                let header = gtk::Label::builder().label(&summary).xalign(0.0).build();
                header.add_css_class("month");
                self.history.append(&header);
                for entry in entries {
                    self.history.append(&self.history_row(entry));
                }
                continue;
            }
            let expander = gtk::Expander::builder().label(&summary).build();
            expander.add_css_class("month-fold");
            let key = (*year, *month);
            let fill = {
                let (me, expander, entries) = (Rc::downgrade(self), expander.clone(), entries.clone());
                move || {
                    let Some(me) = me.upgrade() else { return };
                    if expander.child().is_some() {
                        return;
                    }
                    let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    for entry in &entries {
                        rows.append(&me.history_row(entry));
                    }
                    expander.set_child(Some(&rows));
                }
            };
            if self.opened.borrow().contains(&key) {
                fill();
                expander.set_expanded(true);
            }
            let me = Rc::downgrade(self);
            expander.connect_expanded_notify(move |expander| {
                if let Some(me) = me.upgrade() {
                    if expander.is_expanded() {
                        me.opened.borrow_mut().insert(key);
                    } else {
                        me.opened.borrow_mut().remove(&key);
                    }
                }
                if expander.is_expanded() {
                    fill();
                }
            });
            self.history.append(&expander);
        }
        self.older.set_visible(months.len() > shown);
    }

    /// One reading: when, how much, how much more than the one before, and Edit and Delete. Edit turns the row into
    /// a small form right where it is.
    fn history_row(self: &Rc<Self>, (index, reading, change): &Entry) -> gtk::Box {
        let index = *index;
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let change = change.map_or(String::new(), |change| format!("+{change:.1}"));
        let name = if reading.note.is_empty() { text::day(reading.date) } else { format!("{} · {}", text::day(reading.date), reading.note) };
        let line = row(&format!("{name}    {}", text::kwh(reading.kwh)), &change);
        let edit = small_button("Edit");
        let delete = small_button("Delete");
        let (on_edit, on_delete) = (Rc::downgrade(self), Rc::downgrade(self));
        let place = holder.clone();
        edit.connect_clicked(move |_| {
            if let Some(me) = on_edit.upgrade() {
                me.edit_in_place(&place, index);
            }
        });
        delete.connect_clicked(move |_| {
            if let Some(me) = on_delete.upgrade() {
                me.delete_reading(index);
            }
        });
        line.append(&edit);
        line.append(&delete);
        holder.append(&line);
        holder
    }

    fn refresh_bills(self: &Rc<Self>, all: &[Bill]) {
        clear(&self.bills);
        let mut newest_first: Vec<(usize, &Bill)> = all.iter().enumerate().collect();
        newest_first.sort_by_key(|(index, bill)| std::cmp::Reverse((bill.to, *index)));
        if newest_first.is_empty() {
            self.bills.append(&row("No bills yet.", ""));
        }
        for (index, bill) in newest_first {
            let rate = bill.rate().map_or(String::new(), |rate| format!("₱{rate:.2}/kWh"));
            let line = row(&format!("{} – {}    {}    {}", text::day(bill.from), text::day(bill.to), text::kwh(bill.kwh), bill.amount), &rate);
            let delete = small_button("Delete");
            let me = Rc::downgrade(self);
            delete.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.delete_bill(index);
                }
            });
            line.append(&delete);
            self.bills.append(&line);
        }
    }

    fn show_settings(&self) {
        let settings = self.settings.borrow();
        let config = &settings.electric;
        self.budget.set_text(&config.budget.pesos_f64().to_string());
        self.billing_day.set_text(&config.billing_day().to_string());
        self.rate.set_text(&config.fallback_rate.to_string());
        self.near.set_text(&percent(config.thresholds.near_budget));
        self.comfortable.set_text(&percent(config.thresholds.comfortable));
        self.week.set_text(&percent(config.thresholds.week_rise));
        self.gap.set_text(&percent(config.thresholds.weekday_gap));
        self.over.set_text(&percent(config.thresholds.days_over_share));
    }

    fn say_log(&self, message: &str, error: bool) {
        self.log_note.set_label(message);
        if error {
            self.log_note.add_css_class("error");
        } else {
            self.log_note.remove_css_class("error");
        }
    }

    fn clear_form(&self) {
        self.held.set(None);
        self.kwh.set_text("");
        self.date.set_text("");
    }

    fn add_reading(self: &Rc<Self>) {
        let today = Self::today();
        let result = (|| -> Result<(f64, NaiveDate), String> {
            let kwh = parse_number(&self.kwh.text(), "the meter reading")?;
            let date = match self.date.text().trim() {
                "" => today,
                text => parse_date(text, today)?,
            };
            self.check(&self.store.readings(), date, kwh, today)?;
            self.store.add_reading(&Reading { date, kwh, note: String::new() }).map_err(|e| format!("Could not save: {e}"))?;
            Ok((kwh, date))
        })();
        match result {
            Ok((kwh, date)) => {
                self.clear_form();
                self.say_log(&format!("Logged {} on {}.", text::kwh(kwh), text::day(date)), false);
                self.refresh();
            }
            Err(message) => self.say_log(&message, true),
        }
    }

    /// Checks a new or changed reading against `others`. A day that has not come is refused outright. A meter only
    /// counts up, so a number out of line with its neighbours is most likely a typo: say so, and take it the second
    /// time it is entered unchanged.
    fn check(&self, others: &[Reading], date: NaiveDate, kwh: f64, today: NaiveDate) -> Result<(), String> {
        if date > today {
            return check_reading_in(others, date, kwh, today);
        }
        if self.held.get() != Some((date, kwh))
            && let Err(message) = check_reading_in(others, date, kwh, today)
        {
            self.held.set(Some((date, kwh)));
            return Err(format!("{message} Press Save again to keep it anyway."));
        }
        Ok(())
    }

    /// Turns a history row into a small form in place: the date, the reading, Save and Cancel.
    fn edit_in_place(self: &Rc<Self>, holder: &gtk::Box, index: usize) {
        let all = self.store.readings();
        let Some(reading) = all.get(index) else { return };
        self.held.set(None);
        clear(holder);
        let date = entry("Date", 12);
        date.set_text(&reading.date.to_string());
        let kwh = entry("Meter, kWh", 10);
        kwh.set_text(&reading.kwh.to_string());
        let save = gtk::Button::with_label("Save reading");
        save.add_css_class("suggested-action");
        let cancel = small_button("Cancel");
        let problem = note();
        let form = line(&[date.upcast_ref(), kwh.upcast_ref(), save.upcast_ref(), cancel.upcast_ref()]);
        form.add_css_class("editing");
        holder.append(&form);
        holder.append(&problem);

        let commit = {
            let (me, date, kwh, problem) = (Rc::downgrade(self), date.clone(), kwh.clone(), problem.clone());
            move || {
                let Some(me) = me.upgrade() else { return };
                let today = Self::today();
                let result = (|| -> Result<(f64, NaiveDate), String> {
                    let number = parse_number(&kwh.text(), "the meter reading")?;
                    let day = parse_date(date.text().trim(), today)?;
                    let mut all = me.store.readings();
                    let others: Vec<Reading> = all.iter().enumerate().filter(|(at, _)| *at != index).map(|(_, r)| r.clone()).collect();
                    me.check(&others, day, number, today)?;
                    let target = all.get_mut(index).ok_or("That reading is gone")?;
                    target.date = day;
                    target.kwh = number;
                    me.store.set_readings(&all).map_err(|e| format!("Could not save: {e}"))?;
                    Ok((number, day))
                })();
                match result {
                    Ok((number, day)) => {
                        me.say_log(&format!("Changed to {} on {}.", text::kwh(number), text::day(day)), false);
                        me.refresh();
                    }
                    Err(message) => {
                        problem.set_label(&message);
                        problem.add_css_class("error");
                    }
                }
            }
        };
        let commit = Rc::new(commit);
        for field in [&date, &kwh] {
            let commit = commit.clone();
            field.connect_activate(move |_| commit());
        }
        let on_save = commit.clone();
        save.connect_clicked(move |_| on_save());
        let me = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.refresh();
            }
        });
        kwh.grab_focus();
    }

    fn delete_reading(self: &Rc<Self>, index: usize) {
        let mut all = self.store.readings();
        if index >= all.len() {
            return;
        }
        let gone = all.remove(index);
        match self.store.set_readings(&all) {
            Ok(()) => {
                self.clear_form();
                self.say_log(&format!("Deleted {} on {}.", text::kwh(gone.kwh), text::day(gone.date)), false);
                *self.deleted.borrow_mut() = Some(gone);
                self.restore.set_visible(true);
                self.refresh();
            }
            Err(error) => self.say_log(&format!("Could not delete: {error}"), true),
        }
    }

    fn restore_reading(self: &Rc<Self>) {
        let Some(reading) = self.deleted.borrow_mut().take() else { return };
        self.restore.set_visible(false);
        match self.store.add_reading(&reading) {
            Ok(()) => self.say_log(&format!("Put back {} on {}.", text::kwh(reading.kwh), text::day(reading.date)), false),
            Err(error) => self.say_log(&format!("Could not put it back: {error}"), true),
        }
        self.refresh();
    }

    fn undo(self: &Rc<Self>) {
        let message = match self.store.remove_last_reading() {
            Ok(Some(reading)) => format!("Took back {} on {}.", text::kwh(reading.kwh), text::day(reading.date)),
            Ok(None) => "No readings to take back.".to_string(),
            Err(error) => format!("Could not undo: {error}"),
        };
        self.clear_form();
        self.say_log(&message, false);
        self.refresh();
    }

    fn show_older(self: &Rc<Self>) {
        self.months_shown.set(self.months_shown.get() + MONTHS);
        self.refresh();
    }

    fn record_bill(self: &Rc<Self>) {
        let today = Self::today();
        let result = (|| {
            let kwh = parse_number(&self.bill_kwh.text(), "kWh")?;
            let amount = Money::parse(&self.bill_amount.text()).filter(|m| !m.is_negative()).ok_or("The amount is not a number of pesos")?;
            let from = parse_date(self.bill_from.text().trim(), today)?;
            let to = parse_date(self.bill_to.text().trim(), today)?;
            if kwh <= 0.0 {
                return Err("A bill needs the kWh it covers, to work out the rate".to_string());
            }
            if to < from {
                return Err("The bill ends before it starts".to_string());
            }
            self.store.add_bill(&Bill { from, to, kwh, amount }).map_err(|e| format!("Could not save: {e}"))?;
            Ok(amount)
        })();
        match result {
            Ok(amount) => {
                for field in [&self.bill_kwh, &self.bill_amount, &self.bill_from, &self.bill_to] {
                    field.set_text("");
                }
                self.bill_note.set_label(&format!("Recorded {amount}. The rate now comes from this bill."));
                self.bill_note.remove_css_class("error");
                self.refresh();
            }
            Err(message) => {
                self.bill_note.set_label(&message);
                self.bill_note.add_css_class("error");
            }
        }
    }

    fn delete_bill(self: &Rc<Self>, index: usize) {
        let mut all = self.store.bills();
        if index >= all.len() {
            return;
        }
        let gone = all.remove(index);
        match self.store.set_bills(&all) {
            Ok(()) => {
                self.bill_note.set_label(&format!("Deleted the {} bill. The rate goes back to the last one left, or the placeholder.", gone.amount));
                self.bill_note.remove_css_class("error");
                self.refresh();
            }
            Err(error) => {
                self.bill_note.set_label(&format!("Could not delete: {error}"));
                self.bill_note.add_css_class("error");
            }
        }
    }

    fn show_what_if(&self) {
        let (watts, hours) = (self.watts.text(), self.hours.text());
        if watts.trim().is_empty() && hours.trim().is_empty() {
            self.what_if.set_label("Type an appliance's watts and the hours a day you would use it.");
            return;
        }
        let (Ok(watts), Ok(hours)) = (parse_number(&watts, "watts"), parse_number(&hours, "hours")) else {
            self.what_if.set_label("Watts and hours are numbers.");
            return;
        };
        let plan = self.plan();
        let (kwh, cost) = electric::what_if(watts, hours, plan.rate, plan.cycle.days());
        self.what_if.set_label(&format!("Adds {} and {cost} to a {}-day cycle.", text::kwh(kwh), plan.cycle.days()));
    }

    fn save_settings(self: &Rc<Self>) {
        let result = (|| {
            let budget = Money::parse(&self.budget.text()).filter(|m| !m.is_negative()).ok_or("The budget is not a number of pesos")?;
            let day: u32 = self.billing_day.text().trim().parse().map_err(|_| "The cycle's start is a day of the month, 1 to 28")?;
            if !(1..=28).contains(&day) {
                return Err("The cycle's start is a day of the month, 1 to 28");
            }
            let rate = parse_number(&self.rate.text(), "the rate").map_err(|_| "The rate is pesos per kWh, a number")?;
            // The thresholds are typed as percentages.
            let share = |entry: &gtk::Entry| -> Result<f64, &'static str> {
                match parse_number(&entry.text(), "a threshold") {
                    Ok(value) if value <= 1000.0 => Ok(value / 100.0),
                    _ => Err("Thresholds are percentages, 0 to 1000"),
                }
            };
            let (near, comfortable, week, gap, over) = (share(&self.near)?, share(&self.comfortable)?, share(&self.week)?, share(&self.gap)?, share(&self.over)?);
            if comfortable > near {
                return Err("“Plenty of room” has to be at or below “Close to the budget”");
            }
            let mut settings = self.settings.borrow_mut();
            let config = &mut settings.electric;
            config.budget = budget;
            config.billing_day = day;
            config.fallback_rate = rate;
            config.thresholds.near_budget = near;
            config.thresholds.comfortable = comfortable;
            config.thresholds.week_rise = week;
            config.thresholds.week_drop = week;
            config.thresholds.weekday_gap = gap;
            config.thresholds.days_over_share = over;
            settings.save().map_err(|_| "Could not save your settings")
        })();
        match result {
            Ok(()) => {
                self.settings_note.set_label("Saved.");
                self.settings_note.remove_css_class("error");
                self.refresh();
            }
            Err(message) => {
                self.settings_note.set_label(message);
                self.settings_note.add_css_class("error");
            }
        }
    }
}
