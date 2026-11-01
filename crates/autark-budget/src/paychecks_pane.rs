//! Paychecks: each payday, what falls due out of it, and what is left for you to spend.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_finance::payday::{Line, Payment, Period};
use autark_finance::store::Store;
use gtk::glib;
use gtk::prelude::*;

use crate::settings::Settings;
use crate::text;
use crate::ui::{clear, dim, pesos_text, row, section};
use crate::view;

/// The current payday and the next three, and this many more each time you ask for later ones.
const SHOWN: usize = 4;
const MOST: usize = 48;

type EditHandler = Rc<dyn Fn(&str)>;

pub struct PaychecksPane {
    pub root: gtk::ScrolledWindow,
    body: gtk::Box,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    /// Called with an item's name when its Edit is pressed; the window opens it in Plan.
    edit: RefCell<Option<EditHandler>>,
    shown: Cell<usize>,
}

impl PaychecksPane {
    pub fn new(store: Store, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let (root, body) = crate::ui::page();
        let pane = Rc::new(Self { root, body, store, settings, edit: RefCell::new(None), shown: Cell::new(SHOWN) });
        pane.refresh();
        pane
    }

    pub fn on_edit(&self, handler: impl Fn(&str) + 'static) {
        *self.edit.borrow_mut() = Some(Rc::new(handler));
    }

    pub fn refresh(self: &Rc<Self>) {
        clear(&self.body);
        let settings = self.settings.borrow();
        let today = view::today();
        if settings.finance.paycheck.centavos() <= 0 && settings.finance.items.is_empty() {
            self.body.append(&dim("Nothing planned yet. Open Plan to enter your paycheck and what you pay out of it."));
            return;
        }
        for period in view::periods(&self.store, &settings, today, self.shown.get()) {
            self.body.append(&self.card(&period, today));
        }
        self.past_section(&settings, today);
        if self.shown.get() < MOST {
            let more = gtk::Button::with_label("Show later paydays");
            more.set_halign(gtk::Align::Start);
            more.set_margin_top(10);
            let me = Rc::downgrade(self);
            more.connect_clicked(move |_| {
                let Some(me) = me.upgrade() else { return };
                me.shown.set((me.shown.get() + SHOWN).min(MOST));
                glib::idle_add_local_once(move || me.refresh());
            });
            self.body.append(&more);
        }
    }

    /// How the paydays already over went: what the plan left, what you spent and what you kept.
    fn past_section(&self, settings: &Settings, today: chrono::NaiveDate) {
        let past = autark_finance::payday::history(&settings.finance, &self.store.payments(), &self.store.spending(), today, 8);
        if past.is_empty() {
            return;
        }
        let expander = gtk::Expander::builder().label("Past paydays").margin_top(14).build();
        let list = section("");
        if let Some(heading) = list.first_child() {
            list.remove(&heading);
        }
        list.append(&dim("Worked out from today's plan. Spending counts only if you logged it."));
        for item in &past {
            let span = text::period_title(&item.period);
            let result = if !item.logged {
                format!("{} planned", item.period.left)
            } else if item.kept().is_negative() {
                format!("over by {}", item.kept().abs())
            } else {
                format!("kept {} of {}", item.kept(), item.period.left)
            };
            let line = row(&span, &result);
            if item.logged && item.kept().is_negative() {
                line.add_css_class("over");
            }
            list.append(&line);
        }
        expander.set_child(Some(&list));
        self.body.append(&expander);
    }

    fn card(self: &Rc<Self>, period: &Period, today: chrono::NaiveDate) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("summary");
        card.add_css_class("payday");
        card.set_margin_top(8);

        let title = gtk::Label::builder().label(format!("{}{}", text::period_title(period), if period.current { "  ·  now" } else { "" })).xalign(0.0).build();
        title.add_css_class("heading");
        card.append(&title);

        let left = gtk::Label::builder().xalign(0.0).build();
        left.add_css_class("projected");
        let verdict = gtk::Label::builder().xalign(0.0).wrap(true).build();
        verdict.add_css_class("verdict");
        if period.is_short() {
            left.set_label(&format!("−{}", period.left.abs()));
            verdict.set_label("Short: more falls due than this paycheck brings.");
            verdict.add_css_class("over");
        } else {
            left.set_label(&period.left.to_string());
            verdict.set_label(&format!("yours to spend, {} a day", period.per_day()));
            verdict.add_css_class("under");
        }
        card.append(&left);
        card.append(&verdict);

        card.append(&crate::ui::row("Paycheck", &period.income.to_string()));
        for line in &period.lines {
            card.append(&self.line_row(period, line, today));
        }
        if period.lines.is_empty() {
            card.append(&dim("Nothing falls due this payday."));
        }
        card
    }

    /// What you paid, as plain text with an Edit button. Edit opens a box with Save and Cancel, so a stray click or
    /// keystroke never alters a payment: only Save (or Enter in the box) does, and Cancel or Escape puts it away.
    fn paid_amount(self: &Rc<Self>, row: &gtk::Box, start: chrono::NaiveDate, item: &str, was: autark_finance::money::Money) {
        let amount = gtk::Label::builder().label(was.to_string()).xalign(1.0).width_chars(11).build();
        amount.add_css_class("dim");
        let change = crate::ui::small_button("Edit");
        change.set_tooltip_text(Some("Change what you paid"));
        let entry = gtk::Entry::builder().xalign(1.0).width_chars(9).max_width_chars(9).visible(false).build();
        let save = crate::ui::small_button("Save");
        let cancel = crate::ui::small_button("Cancel");
        entry.connect_changed(|entry| {
            entry.remove_css_class("error");
            entry.set_tooltip_text(None);
        });
        save.set_visible(false);
        cancel.set_visible(false);
        for widget in [amount.upcast_ref::<gtk::Widget>(), change.upcast_ref(), entry.upcast_ref(), save.upcast_ref(), cancel.upcast_ref()] {
            row.append(widget);
        }

        let close = {
            let (amount, change, entry, save, cancel) = (amount.clone(), change.clone(), entry.clone(), save.clone(), cancel.clone());
            Rc::new(move || {
                for widget in [entry.upcast_ref::<gtk::Widget>(), save.upcast_ref(), cancel.upcast_ref()] {
                    widget.set_visible(false);
                }
                amount.set_visible(true);
                change.set_visible(true);
            })
        };
        {
            let (amount, change, entry, save, cancel) = (amount.clone(), change.clone(), entry.clone(), save.clone(), cancel.clone());
            change.connect_clicked(move |change| {
                entry.set_text(&pesos_text(was));
                for widget in [entry.upcast_ref::<gtk::Widget>(), save.upcast_ref(), cancel.upcast_ref()] {
                    widget.set_visible(true);
                }
                amount.set_visible(false);
                change.set_visible(false);
                entry.grab_focus();
            });
        }
        let commit = {
            let (me, entry, item, close) = (Rc::downgrade(self), entry.clone(), item.to_string(), close.clone());
            Rc::new(move || {
                let Some(me) = me.upgrade() else { return };
                // Only an open box can change a payment.
                if !entry.get_visible() {
                    return;
                }
                let text = entry.text();
                let Some(amount) = autark_finance::money::Money::parse(text.trim()).filter(|m| m.centavos() > 0) else {
                    // Not an amount: leave the box open to fix, and say so.
                    entry.add_css_class("error");
                    entry.set_tooltip_text(Some("That is not an amount"));
                    return;
                };
                if amount != was {
                    let mut all = me.store.payments();
                    let mut first = true;
                    all.retain_mut(|p| {
                        if p.period != start || !p.item.eq_ignore_ascii_case(&item) {
                            return true;
                        }
                        // One payment holds the whole amount.
                        std::mem::take(&mut first).then(|| p.amount = amount).is_some()
                    });
                    let _ = me.store.set_payments(&all);
                    glib::idle_add_local_once(move || me.refresh());
                } else {
                    close();
                }
            })
        };
        let on_enter = commit.clone();
        entry.connect_activate(move |_| on_enter());
        save.connect_clicked(move |_| commit());
        let on_cancel = close.clone();
        cancel.connect_clicked(move |_| on_cancel());
        let escape = gtk::EventControllerKey::new();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(escape);
    }

    fn line_row(self: &Rc<Self>, period: &Period, line: &Line, today: chrono::NaiveDate) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("row");
        let check = gtk::CheckButton::builder().active(line.paid).tooltip_text("Tick when you have paid it").build();
        let name = gtk::Label::builder().label(&line.name).xalign(0.0).hexpand(true).build();
        let due = gtk::Label::builder().label(format!("due {}", text::day(line.due))).build();
        due.add_css_class("dim");
        row.append(&check);
        row.append(&name);
        row.append(&due);
        if line.paid {
            name.add_css_class("dim");
            self.paid_amount(&row, period.start, &line.name, line.amount);
        } else {
            let amount = gtk::Label::builder().label(format!("{}{}", text::confidence_mark(line.confidence), line.amount)).xalign(1.0).width_chars(11).build();
            row.append(&amount);
        }
        // One Edit on every line. On a line still to pay it opens the item in Plan; on a paid line the Edit is the one
        // beside the amount, for what you paid. Electricity is not an item of the plan (its amount comes from the
        // meter), so it has none.
        if !line.paid && self.settings.borrow().finance.find(&line.name).is_some() {
            let edit = crate::ui::small_button("Edit");
            let (me, name) = (Rc::downgrade(self), line.name.clone());
            edit.connect_clicked(move |_| {
                let handler = me.upgrade().and_then(|me| me.edit.borrow().clone());
                if let Some(handler) = handler {
                    handler(&name);
                }
            });
            row.append(&edit);
        }

        let (me, start, item, owed) = (Rc::downgrade(self), period.start, line.name.clone(), line.amount);
        check.connect_toggled(move |check| {
            let Some(me) = me.upgrade() else { return };
            let mut all = me.store.payments();
            all.retain(|p| !(p.period == start && p.item.eq_ignore_ascii_case(&item)));
            if check.is_active() {
                all.push(Payment { item: item.clone(), period: start, date: today, amount: owed });
            }
            let _ = me.store.set_payments(&all);
            // Rebuilding inside the toggle would destroy the button that is still handling it.
            glib::idle_add_local_once(move || me.refresh());
        });
        row
    }
}
