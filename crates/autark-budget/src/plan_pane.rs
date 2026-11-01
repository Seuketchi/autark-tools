//! Plan: your paycheck and everything you pay out of it, as cards. Adding or changing one opens a small form that asks
//! only for what that kind of thing needs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_finance::money::Money;
use autark_finance::payday::{self, Forecast};
use autark_finance::plan::{Every, Item, Kind};
use autark_finance::store::Store;
use gtk::glib;
use gtk::prelude::*;

use crate::cli::parse_date;
use crate::settings::Settings;
use crate::text;
use crate::ui::{clear, dim, entry, line, note, section, small_button};
use crate::view;

/// In the order of `Kind::ALL`.
const KINDS: [&str; 4] = ["Bill", "Goal: saving up", "Debt: paying back", "Installment: fixed payments"];
const EVERY: [&str; 3] = ["Every payday", "Every month", "Once"];

#[derive(Clone, Debug, PartialEq)]
enum Editing {
    Nothing,
    New,
    Item(String),
}

/// An entry or dropdown with its label above it, so the label stays once the box is filled in.
struct Field {
    wrap: gtk::Box,
    label: gtk::Label,
}

fn field(label: &str, widget: &impl IsA<gtk::Widget>) -> Field {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let label = gtk::Label::builder().label(label).xalign(0.0).build();
    label.add_css_class("form-label");
    wrap.append(&label);
    wrap.append(widget);
    Field { wrap, label }
}

pub struct PlanPane {
    pub root: gtk::ScrolledWindow,
    body: gtk::Box,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    editing: RefCell<Editing>,
    status: gtk::Label,
    status_row: gtk::Box,
    undo: gtk::Button,
    /// The item you just deleted and where it was, so Undo can put it back.
    deleted: RefCell<Option<(usize, Item)>>,
    categories: gtk::Entry,
    categories_note: gtk::Label,

    pay_open: Cell<bool>,
    pay_form: gtk::Box,
    paycheck: gtk::Entry,
    pay_days: gtk::Entry,
    pay_note: gtk::Label,

    form: gtk::Box,
    form_title: gtk::Label,
    form_note: gtk::Label,
    name: gtk::Entry,
    kind: gtk::DropDown,
    every: gtk::DropDown,
    amount: gtk::Entry,
    day: gtk::Entry,
    date: gtk::Entry,
    times: gtk::Entry,
    target: gtk::Entry,
    saved: gtk::Entry,
    amount_field: Field,
    every_field: Field,
    day_field: Field,
    date_field: Field,
    times_field: Field,
    target_field: Field,
    saved_field: Field,
}

impl PlanPane {
    pub fn new(store: Store, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let (root, body) = crate::ui::page();
        let name = entry("e.g. Rent", 18);
        let kind = gtk::DropDown::from_strings(&KINDS);
        let every = gtk::DropDown::from_strings(&EVERY);
        let amount = entry("e.g. 4000", 12);
        let day = entry("1 to 31", 8);
        let date = entry("YYYY-MM-DD", 13);
        let times = entry("e.g. 3", 6);
        let target = entry("e.g. 25000", 12);
        let saved = entry("e.g. 12000", 12);
        let paycheck = entry("e.g. 12000", 14);
        let pay_days = entry("e.g. 15,30", 12);
        let (amount_field, every_field, day_field, date_field, times_field, target_field, saved_field) =
            (field("Amount, ₱", &amount), field("How often", &every), field("Day of the month (31 = last day)", &day), field("Date", &date), field("How many times", &times), field("Goal, ₱", &target), field("Saved so far, ₱", &saved));
        every.set_selected(1);

        let pane = Rc::new(Self {
            root,
            body,
            store,
            settings,
            editing: RefCell::new(Editing::Nothing),
            status: note(),
            status_row: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            undo: small_button("Undo"),
            deleted: RefCell::new(None),
            categories: entry("e.g. Food, Transport, Load", 30),
            categories_note: note(),
            pay_open: Cell::new(false),
            pay_form: gtk::Box::new(gtk::Orientation::Vertical, 8),
            paycheck,
            pay_days,
            pay_note: note(),
            form: gtk::Box::new(gtk::Orientation::Vertical, 10),
            form_title: gtk::Label::builder().xalign(0.0).build(),
            form_note: note(),
            name,
            kind,
            every,
            amount,
            day,
            date,
            times,
            target,
            saved,
            amount_field,
            every_field,
            day_field,
            date_field,
            times_field,
            target_field,
            saved_field,
        });
        pane.build_forms();
        pane.status_row.append(&pane.status);
        pane.status_row.append(&pane.undo);
        let me = Rc::downgrade(&pane);
        pane.undo.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.undo_delete();
            }
        });
        pane.adapt();
        pane.refresh();
        pane
    }

    fn build_forms(self: &Rc<Self>) {
        // The paycheck's own small editor.
        let pay = &self.pay_form;
        let (each, days) = (field("Each payday, ₱", &self.paycheck), field("Paydays (days of the month)", &self.pay_days));
        pay.append(&line(&[each.wrap.upcast_ref(), days.wrap.upcast_ref()]));
        let (save, cancel) = (gtk::Button::with_label("Save"), small_button("Cancel"));
        save.add_css_class("suggested-action");
        pay.append(&line(&[save.upcast_ref(), cancel.upcast_ref()]));
        pay.append(&self.pay_note);
        let me = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.save_pay();
            }
        });
        let me = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.pay_open.set(false);
                me.refresh();
            }
        });

        // The item form: what it is, then only the boxes that kind needs.
        let form = &self.form;
        form.add_css_class("item-card");
        self.form_title.add_css_class("heading");
        form.append(&self.form_title);
        let (kind, name) = (field("What is it?", &self.kind), field("Name", &self.name));
        form.append(&line(&[kind.wrap.upcast_ref(), name.wrap.upcast_ref()]));
        form.append(&line(&[self.amount_field.wrap.upcast_ref(), self.every_field.wrap.upcast_ref()]));
        form.append(&line(&[self.day_field.wrap.upcast_ref(), self.date_field.wrap.upcast_ref(), self.times_field.wrap.upcast_ref()]));
        form.append(&line(&[self.target_field.wrap.upcast_ref(), self.saved_field.wrap.upcast_ref()]));
        let (save, cancel) = (gtk::Button::with_label("Save"), small_button("Cancel"));
        save.add_css_class("suggested-action");
        form.append(&line(&[save.upcast_ref(), cancel.upcast_ref()]));
        form.append(&self.form_note);

        let me = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.save_item();
            }
        });
        let me = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                *me.editing.borrow_mut() = Editing::Nothing;
                me.refresh();
            }
        });
        let me = Rc::downgrade(self);
        self.categories.connect_activate(move |_| {
            if let Some(me) = me.upgrade() {
                me.save_categories();
            }
        });
        for dropdown in [&self.kind, &self.every] {
            let me = Rc::downgrade(self);
            dropdown.connect_selected_notify(move |_| {
                if let Some(me) = me.upgrade() {
                    me.adapt();
                }
            });
        }
        for entry in [&self.name, &self.amount, &self.day, &self.date, &self.times, &self.target, &self.saved] {
            let me = Rc::downgrade(self);
            entry.connect_activate(move |_| {
                if let Some(me) = me.upgrade() {
                    me.save_item();
                }
            });
        }
    }

    fn kind(&self) -> Kind {
        Kind::ALL.get(self.kind.selected() as usize).copied().unwrap_or_default()
    }

    /// How often it comes up: a goal is always put in each payday, an installment is always monthly.
    fn every(&self) -> Every {
        match self.kind() {
            Kind::Goal => Every::Payday,
            Kind::Installment => Every::Month,
            _ => [Every::Payday, Every::Month, Every::Once].get(self.every.selected() as usize).copied().unwrap_or_default(),
        }
    }

    /// Shows only the boxes the chosen kind needs.
    fn adapt(&self) {
        let (kind, every) = (self.kind(), self.every());
        self.every_field.wrap.set_visible(matches!(kind, Kind::Bill | Kind::Debt));
        self.day_field.wrap.set_visible(every == Every::Month);
        self.date_field.wrap.set_visible(kind != Kind::Goal);
        self.date_field.label.set_label(match (kind, every) {
            (_, Every::Once) => "Due date",
            (Kind::Installment, _) => "First payment",
            _ => "Starts (optional)",
        });
        self.times_field.wrap.set_visible(matches!(kind, Kind::Debt | Kind::Installment) && every != Every::Once);
        self.target_field.wrap.set_visible(kind == Kind::Goal);
        self.saved_field.wrap.set_visible(kind == Kind::Goal);
        self.amount_field.label.set_label(match kind {
            Kind::Goal => "Put in each payday, ₱",
            Kind::Installment => "Each payment, ₱",
            _ => "Amount, ₱",
        });
    }

    fn persist(&self) -> Result<(), String> {
        self.settings.borrow().save().map_err(|_| "Could not save your settings".to_string())
    }

    fn say(label: &gtk::Label, message: &str, error: bool) {
        if error {
            label.add_css_class("error");
        } else {
            label.remove_css_class("error");
        }
        label.set_label(message);
    }

    /// Says something in the status line. It also lets go of a deleted item: Undo is only for the last thing done.
    fn announce(&self, message: &str) {
        *self.deleted.borrow_mut() = None;
        Self::say(&self.status, message, false);
    }

    fn undo_delete(self: &Rc<Self>) {
        let Some((at, item)) = self.deleted.borrow_mut().take() else { return };
        let name = item.name.clone();
        let result = (|| -> Result<(), String> {
            let mut config = self.settings.borrow().finance.clone();
            config.put(item)?;
            // `put` adds at the end; move it back to where it was.
            if let Some(from) = config.find(&name) {
                let item = config.items.remove(from);
                config.items.insert(at.min(config.items.len()), item);
            }
            self.settings.borrow_mut().finance = config;
            self.persist()
        })();
        match result {
            Ok(()) => Self::say(&self.status, &format!("Put {name} back."), false),
            Err(message) => Self::say(&self.status, &message, true),
        }
        self.refresh();
    }

    fn save_categories(self: &Rc<Self>) {
        let result = (|| -> Result<(), String> {
            let mut config = self.settings.borrow().finance.clone();
            config.set_categories(&self.categories.text())?;
            self.settings.borrow_mut().finance = config;
            self.persist()
        })();
        match result {
            Ok(()) => {
                Self::say(&self.categories_note, "Saved. They show on the Spending pane.", false);
                self.refresh();
            }
            Err(message) => Self::say(&self.categories_note, &message, true),
        }
    }

    fn back_up(&self) {
        let today = view::today();
        let result = (|| -> Result<String, String> {
            let to = crate::backup::default_folder(today).ok_or("There is no home folder to put it in")?;
            let config = autark_common::config::paths("budget").into_iter().next().ok_or("There is no config folder")?;
            let copied = crate::backup::back_up(&to, self.store.dir(), &config).map_err(|e| format!("Could not back up: {e}"))?;
            Ok(format!("Backed up {} files to {}.", copied.len(), to.display()))
        })();
        match result {
            Ok(message) => self.announce(&message),
            Err(message) => Self::say(&self.status, &message, true),
        }
    }

    fn save_pay(self: &Rc<Self>) {
        let result = (|| -> Result<(), String> {
            let text = self.paycheck.text();
            let paycheck = Money::parse(text.trim()).filter(|m| !m.is_negative()).ok_or_else(|| format!("“{}” is not an amount", text.trim()))?;
            let days: Result<Vec<u32>, _> = self.pay_days.text().split(',').map(|d| d.trim().parse::<u32>()).collect();
            let days = days.map_err(|_| "Paydays are days of the month, like 15,30".to_string())?;
            if days.is_empty() || days.iter().any(|d| !(1..=31).contains(d)) {
                return Err("Paydays are days of the month, 1 to 31".into());
            }
            let mut settings = self.settings.borrow_mut();
            settings.finance.paycheck = paycheck;
            settings.finance.pay_days = days;
            drop(settings);
            self.persist()
        })();
        match result {
            Ok(()) => {
                self.pay_open.set(false);
                self.announce("Saved your paycheck.");
                self.refresh();
            }
            Err(message) => Self::say(&self.pay_note, &message, true),
        }
    }

    fn clear_form(&self) {
        for field in [&self.name, &self.amount, &self.day, &self.date, &self.times, &self.target, &self.saved] {
            field.set_text("");
        }
        self.kind.set_selected(0);
        self.every.set_selected(1);
        Self::say(&self.form_note, "", false);
        self.adapt();
    }

    fn read_item(&self) -> Result<Item, String> {
        let today = view::today();
        let (kind, every) = (self.kind(), self.every());
        let money = |entry: &gtk::Entry| -> Result<Option<Money>, String> {
            let text = entry.text();
            let text = text.trim();
            if text.is_empty() { Ok(None) } else { Money::parse(text).map(Some).ok_or_else(|| format!("“{text}” is not an amount")) }
        };
        let day = match self.day.text().trim() {
            "" => None,
            text => Some(text.parse::<u32>().map_err(|_| format!("“{text}” is not a day of the month"))?),
        };
        let date = match self.date.text().trim() {
            "" => None,
            text => Some(parse_date(text, today)?),
        };
        let times = match self.times.text().trim() {
            "" => None,
            text => Some(text.parse::<u32>().map_err(|_| format!("“{text}” is not a number of times"))?),
        };
        let goal = kind == Kind::Goal;
        let item = Item {
            name: self.name.text().trim().to_string(),
            kind,
            amount: money(&self.amount)?.unwrap_or_default(),
            every,
            day: if every == Every::Month { day } else { None },
            date: if goal { None } else { date },
            times: if matches!(kind, Kind::Debt | Kind::Installment) && every != Every::Once { times } else { None },
            target: if goal { money(&self.target)? } else { None },
            saved: if goal { money(&self.saved)?.unwrap_or_default() } else { Money::ZERO },
        };
        item.check()?;
        Ok(item)
    }

    fn save_item(self: &Rc<Self>) {
        let editing = self.editing.borrow().clone();
        let result = (|| -> Result<String, String> {
            let item = self.read_item()?;
            let name = item.name.clone();
            let mut config = self.settings.borrow().finance.clone();
            let replaced = config.find(&name).is_some() || matches!(&editing, Editing::Item(_));
            if let Editing::Item(old) = &editing {
                config.remove(old);
            }
            config.put(item)?;
            self.settings.borrow_mut().finance = config;
            self.persist()?;
            Ok(format!("{} {name}.", if replaced { "Updated" } else { "Added" }))
        })();
        match result {
            Ok(message) => {
                *self.editing.borrow_mut() = Editing::Nothing;
                self.announce(&message);
                self.refresh();
            }
            Err(message) => Self::say(&self.form_note, &message, true),
        }
    }

    fn start_new(self: &Rc<Self>) {
        self.clear_form();
        self.form_title.set_label("Add something");
        *self.editing.borrow_mut() = Editing::New;
        self.announce("");
        self.refresh();
        self.name.grab_focus();
    }

    /// Opens the item of that name in the form, on its own card, if the plan has one.
    pub fn edit_named(self: &Rc<Self>, name: &str) {
        let item = {
            let settings = self.settings.borrow();
            settings.finance.find(name).map(|at| settings.finance.items[at].clone())
        };
        if let Some(item) = item {
            self.edit(&item);
        }
    }

    fn edit(self: &Rc<Self>, item: &Item) {
        self.clear_form();
        self.name.set_text(&item.name);
        self.kind.set_selected(Kind::ALL.iter().position(|k| *k == item.kind).unwrap_or(0) as u32);
        self.every.set_selected(match item.every {
            Every::Payday => 0,
            Every::Month => 1,
            Every::Once => 2,
        });
        let pesos = |m: Money| format!("{:.2}", m.pesos_f64());
        self.amount.set_text(&pesos(item.amount));
        self.day.set_text(&item.day.map_or(String::new(), |d| d.to_string()));
        self.date.set_text(&item.date.map_or(String::new(), |d| d.to_string()));
        self.times.set_text(&item.times.map_or(String::new(), |t| t.to_string()));
        self.target.set_text(&item.target.map_or(String::new(), pesos));
        self.saved.set_text(&if item.saved.centavos() > 0 { pesos(item.saved) } else { String::new() });
        self.form_title.set_label(&format!("Changing {}", item.name));
        *self.editing.borrow_mut() = Editing::Item(item.name.clone());
        self.announce("");
        self.adapt();
        self.refresh();
        self.amount.grab_focus();
    }

    fn open_pay(self: &Rc<Self>) {
        {
            let settings = self.settings.borrow();
            let config = &settings.finance;
            self.paycheck.set_text(&if config.paycheck.centavos() > 0 { format!("{:.2}", config.paycheck.pesos_f64()) } else { String::new() });
            self.pay_days.set_text(&config.pay_days.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
        }
        Self::say(&self.pay_note, "", false);
        self.pay_open.set(true);
        self.refresh();
        self.paycheck.grab_focus();
    }

    /// The forms live on whatever card is open, so they are taken off before the cards are cleared.
    fn detach(&self) {
        for form in [self.form.upcast_ref::<gtk::Widget>(), self.pay_form.upcast_ref(), self.status_row.upcast_ref(), self.categories.upcast_ref(), self.categories_note.upcast_ref()] {
            if let Some(parent) = form.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) {
                parent.remove(form);
            }
        }
    }

    pub fn refresh(self: &Rc<Self>) {
        self.detach();
        clear(&self.body);
        let settings = self.settings.borrow();
        let config = &settings.finance;
        let editing = self.editing.borrow().clone();

        self.body.append(&self.paycheck_card(config.paycheck, &config.pay_days));
        self.undo.set_visible(self.deleted.borrow().is_some());
        self.body.append(&self.status_row);

        if editing == Editing::New {
            self.body.append(&self.form);
        } else {
            let add = gtk::Button::with_label("Add something");
            add.add_css_class("suggested-action");
            add.set_halign(gtk::Align::Start);
            add.set_margin_top(12);
            let me = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.start_new();
                }
            });
            self.body.append(&add);
        }

        if config.items.is_empty() && editing != Editing::New {
            self.body.append(&dim("Nothing planned yet. Add your rent, bills and what you are saving for."));
        }
        let forecasts = payday::forecasts(config, &self.store.payments(), view::today());
        for (title, kinds) in [("Bills", &[Kind::Bill][..]), ("Goals", &[Kind::Goal]), ("Debts and installments", &[Kind::Debt, Kind::Installment])] {
            let items: Vec<&Item> = config.items.iter().filter(|item| kinds.contains(&item.kind)).collect();
            if items.is_empty() {
                continue;
            }
            let group = section(title);
            for item in items {
                if matches!(&editing, Editing::Item(name) if name.eq_ignore_ascii_case(&item.name)) {
                    group.append(&self.form);
                } else {
                    group.append(&self.card(item, forecasts.iter().find(|f| f.name == item.name)));
                }
            }
            self.body.append(&group);
        }

        let buttons = section("Spending buttons");
        buttons.append(&dim("The buttons on the Spending pane, separated by commas. Other is always there."));
        self.categories.set_text(&config.categories.join(", "));
        let save = gtk::Button::with_label("Save");
        buttons.append(&line(&[self.categories.upcast_ref(), save.upcast_ref()]));
        buttons.append(&self.categories_note);
        self.body.append(&buttons);
        let me = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.save_categories();
            }
        });

        let backup = section("Backup");
        backup.append(&dim("Copies your settings and every log to a dated folder in ~/Backups/autark-budget."));
        let button = gtk::Button::with_label("Back up now");
        button.set_halign(gtk::Align::Start);
        backup.append(&button);
        self.body.append(&backup);
        let me = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.back_up();
            }
        });
    }

    fn paycheck_card(self: &Rc<Self>, paycheck: Money, pay_days: &[u32]) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("item-card");
        if self.pay_open.get() {
            let title = gtk::Label::builder().label("Your paycheck").xalign(0.0).build();
            title.add_css_class("heading");
            card.append(&title);
            card.append(&self.pay_form);
            return card;
        }
        let name = gtk::Label::builder().label("Paycheck").xalign(0.0).hexpand(true).build();
        name.add_css_class("item-name");
        let amount = gtk::Label::builder().label(paycheck.to_string()).build();
        amount.add_css_class("item-amount");
        let change = small_button("Change");
        let me = Rc::downgrade(self);
        change.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.open_pay();
            }
        });
        card.append(&line(&[name.upcast_ref(), amount.upcast_ref(), change.upcast_ref()]));
        card.append(&dim(&if paycheck.centavos() > 0 {
            format!("Paid on the {} of each month.", pay_days.iter().map(u32::to_string).collect::<Vec<_>>().join(" and "))
        } else {
            "Set what each payday brings, so every payday can say what is left.".to_string()
        }));
        card
    }

    fn card(self: &Rc<Self>, item: &Item, forecast: Option<&Forecast>) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("item-card");
        let name = gtk::Label::builder().label(&item.name).xalign(0.0).hexpand(true).build();
        name.add_css_class("item-name");
        let amount = gtk::Label::builder().label(item.amount.to_string()).build();
        amount.add_css_class("item-amount");
        let (edit, delete) = (small_button("Edit"), small_button("Delete"));
        card.append(&line(&[name.upcast_ref(), amount.upcast_ref(), edit.upcast_ref(), delete.upcast_ref()]));

        let mut when = item.schedule();
        if let Some(times) = item.times.filter(|_| item.every == Every::Month) {
            when.push_str(&format!(", {times} times"));
        }
        card.append(&dim(&when));
        if let Some(forecast) = forecast.filter(|_| item.kind != Kind::Bill) {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(if forecast.total.centavos() > 0 { (forecast.paid.pesos_f64() / forecast.total.pesos_f64()).clamp(0.0, 1.0) } else { 0.0 });
            card.append(&bar);
            card.append(&dim(&text::progress_line(forecast)));
        }

        let (me, copy) = (Rc::downgrade(self), item.clone());
        edit.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.edit(&copy);
            }
        });
        let (me, name) = (Rc::downgrade(self), item.name.clone());
        delete.connect_clicked(move |_| {
            let Some(me) = me.upgrade() else { return };
            let at = me.settings.borrow().finance.find(&name);
            let gone = me.settings.borrow_mut().finance.remove(&name);
            let _ = me.persist();
            Self::say(&me.status, &format!("Deleted {name}."), false);
            *me.deleted.borrow_mut() = at.zip(gone);
            glib::idle_add_local_once(move || me.refresh());
        });
        card
    }
}
