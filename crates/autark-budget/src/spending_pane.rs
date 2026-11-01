//! Spending: what you have spent this payday, and what is left for today.

use std::cell::RefCell;
use std::rc::Rc;

use autark_finance::money::Money;
use autark_finance::spending::{self, Spend};
use autark_finance::store::Store;
use gtk::glib;
use gtk::prelude::*;

use crate::settings::Settings;
use crate::text;
use crate::ui::{clear, dim, entry, line, note, row, section, small_button};
use crate::view;

pub struct SpendingPane {
    pub root: gtk::ScrolledWindow,
    store: Store,
    settings: Rc<RefCell<Settings>>,
    today: gtk::Label,
    verdict: gtk::Label,
    facts: gtk::Label,
    amount: gtk::Entry,
    detail: gtk::Entry,
    buttons: gtk::Box,
    price: gtk::Entry,
    answer: gtk::Label,
    /// The categories the buttons were last built for.
    built_for: RefCell<Vec<String>>,
    message: gtk::Label,
    categories: gtk::Box,
    list: gtk::Box,
}

impl SpendingPane {
    pub fn new(store: Store, settings: Rc<RefCell<Settings>>) -> Rc<Self> {
        let (root, page) = crate::ui::page();
        let wrapped = || gtk::Label::builder().xalign(0.0).wrap(true).build();
        let pane = Rc::new(Self {
            root,
            store,
            settings,
            today: gtk::Label::builder().xalign(0.0).build(),
            verdict: wrapped(),
            facts: wrapped(),
            amount: entry("Amount, ₱", 12),
            detail: entry("What for (optional)", 22),
            price: entry("Price, ₱", 12),
            answer: wrapped(),
            buttons: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            built_for: RefCell::new(Vec::new()),
            message: note(),
            categories: crate::ui::rows(),
            list: crate::ui::rows(),
        });

        let summary = gtk::Box::new(gtk::Orientation::Vertical, 4);
        summary.add_css_class("summary");
        pane.today.add_css_class("projected");
        pane.verdict.add_css_class("verdict");
        pane.facts.add_css_class("dim");
        summary.append(&dim("Left to spend today"));
        summary.append(&pane.today);
        summary.append(&pane.verdict);
        summary.append(&pane.facts);
        page.append(&summary);

        let ask = section("Can I afford…?");
        ask.append(&dim("Type a price to see if it fits in what this payday has left, and what it does to each day."));
        let check = gtk::Button::with_label("Check");
        ask.append(&line(&[pane.price.upcast_ref(), check.upcast_ref()]));
        pane.answer.add_css_class("verdict");
        pane.answer.set_visible(false);
        ask.append(&pane.answer);
        page.append(&ask);
        let me = Rc::downgrade(&pane);
        check.connect_clicked(move |_| {
            if let Some(me) = me.upgrade() {
                me.check_price();
            }
        });
        let me = Rc::downgrade(&pane);
        pane.price.connect_activate(move |_| {
            if let Some(me) = me.upgrade() {
                me.check_price();
            }
        });

        let log = section("I spent…");
        log.append(&dim("Type the amount, then tap what it was for. Enter logs it as Other."));
        log.append(&line(&[pane.amount.upcast_ref(), pane.detail.upcast_ref()]));
        log.append(&pane.buttons);
        log.append(&pane.message);
        page.append(&log);
        for field in [&pane.amount, &pane.detail] {
            let me = Rc::downgrade(&pane);
            field.connect_activate(move |_| {
                if let Some(me) = me.upgrade() {
                    me.log("Other");
                }
            });
        }

        let by = section("By category");
        by.append(&pane.categories);
        page.append(&by);
        let all = section("This payday");
        all.append(&pane.list);
        page.append(&all);

        pane.refresh();
        pane
    }

    /// Whether the price fits in what this payday has left.
    fn check_price(&self) {
        let text = self.price.text();
        for class in ["under", "over"] {
            self.answer.remove_css_class(class);
        }
        self.answer.set_visible(true);
        let Some(amount) = Money::parse(text.trim()).filter(|m| m.centavos() > 0) else {
            self.answer.add_css_class("over");
            self.answer.set_label(&format!("“{}” is not a price", text.trim()));
            return;
        };
        match view::afford(&self.store, &self.settings.borrow(), amount, view::today()) {
            Some(verdict) => {
                if !verdict.fits {
                    self.answer.add_css_class("over");
                } else if !verdict.tight {
                    self.answer.add_css_class("under");
                }
                self.answer.set_label(&text::afford_text(&verdict));
            }
            None => self.answer.set_label("Set your paycheck and paydays in Plan first."),
        }
    }

    /// One button for each of your categories, then Other. They follow what Plan says, so they are rebuilt each time.
    fn rebuild_buttons(self: &Rc<Self>) {
        let mut names = self.settings.borrow().finance.categories.clone();
        names.push("Other".to_string());
        // A button that is being pressed must not be rebuilt under the press, so only a change rebuilds them.
        if *self.built_for.borrow() == names {
            return;
        }
        clear(&self.buttons);
        *self.built_for.borrow_mut() = names.clone();
        for name in names {
            let button = gtk::Button::with_label(&name);
            let me = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.log(&name);
                }
            });
            self.buttons.append(&button);
        }
    }

    fn log(self: &Rc<Self>, category: &str) {
        let result = (|| -> Result<Money, String> {
            let text = self.amount.text();
            let amount = Money::parse(text.trim()).filter(|m| m.centavos() > 0).ok_or_else(|| format!("“{}” is not an amount", text.trim()))?;
            let note = self.detail.text().trim().to_string();
            let category = if category == "Other" { String::new() } else { category.to_string() };
            self.store.add_spending(&Spend { date: view::today(), amount, category, note }).map_err(|e| format!("Could not save: {e}"))?;
            Ok(amount)
        })();
        match result {
            Ok(amount) => {
                self.amount.set_text("");
                self.detail.set_text("");
                self.refresh();
                let today = view::today();
                let over = view::current(&self.store, &self.settings.borrow(), today)
                    .map(|period| spending::summarize(&period, &self.store.spending(), today))
                    .filter(|summary| summary.today_left.is_negative());
                match over {
                    Some(summary) => {
                        self.message.add_css_class("error");
                        self.message.set_label(&format!("Logged {amount}. That is {} over today's share; each day after gets {} if you spend nothing more.", summary.today_left.abs(), summary.per_day_after));
                    }
                    None => {
                        self.message.remove_css_class("error");
                        self.message.set_label(&format!("Logged {amount}."));
                    }
                }
                self.amount.grab_focus();
            }
            Err(message) => {
                self.message.add_css_class("error");
                self.message.set_label(&message);
            }
        }
    }

    pub fn refresh(self: &Rc<Self>) {
        self.rebuild_buttons();
        clear(&self.categories);
        clear(&self.list);
        let today = view::today();
        let Some(period) = view::current(&self.store, &self.settings.borrow(), today) else {
            self.today.set_label("—");
            self.verdict.set_label("");
            self.facts.set_label("Set your paycheck and paydays in Plan first.");
            return;
        };
        let all = self.store.spending();
        let summary = spending::summarize(&period, &all, today);

        self.today.set_label(&summary.today_left.to_string());
        for class in ["under", "over"] {
            self.verdict.remove_css_class(class);
        }
        if summary.today_left.is_negative() {
            self.verdict.set_label(&format!("over today's {} by {}", summary.today_budget, summary.today_left.abs()));
            self.verdict.add_css_class("over");
        } else {
            self.verdict.set_label(&format!("of today's {}", summary.today_budget));
            self.verdict.add_css_class("under");
        }
        let days = if summary.days_left == 1 { "last day of this payday".to_string() } else { format!("{} days to go", summary.days_left) };
        self.facts.set_label(&format!("{} left of {} this payday, {days}.{}", summary.left, summary.to_spend, if summary.days_left > 1 { format!(" Tomorrow on: {} a day.", summary.per_day_after) } else { String::new() }));

        if summary.by_category.is_empty() {
            self.categories.append(&row("Nothing spent yet this payday.", ""));
        }
        for (name, total) in &summary.by_category {
            self.categories.append(&row(name, &total.to_string()));
        }

        let mut mine: Vec<(usize, &Spend)> = all.iter().enumerate().filter(|(_, s)| s.date >= period.start && s.date < period.end).collect();
        mine.sort_by_key(|(at, s)| (std::cmp::Reverse(s.date), std::cmp::Reverse(*at)));
        for (at, spend) in mine {
            let what = if spend.category.is_empty() { "Other" } else { &spend.category };
            let text = if spend.note.is_empty() { what.to_string() } else { format!("{what} · {}", spend.note) };
            let item = row(&format!("{}  {text}", text::day(spend.date)), &spend.amount.to_string());
            let delete = small_button("Delete");
            let me = Rc::downgrade(self);
            delete.connect_clicked(move |_| {
                let Some(me) = me.upgrade() else { return };
                let mut all = me.store.spending();
                if at < all.len() {
                    all.remove(at);
                    let _ = me.store.set_spending(&all);
                }
                glib::idle_add_local_once(move || me.refresh());
            });
            item.append(&delete);
            self.list.append(&item);
        }
    }
}
