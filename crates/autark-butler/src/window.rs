//! The window: a sidebar of pages (Approvals, Memory, Today, Turns, Linked) and a refresh button. Every page is
//! rebuilt from one round trip through the door; a yes, a no or a forget is one more trip, then a refresh.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde_json::{Value, json};

use crate::door::{self, Door};

/// The lists `alfred window` will show under Today; it refuses anything else.
const TODAY: [&str; 11] =
    ["reminders", "calendar", "tasks", "habits", "goals", "chores", "groceries", "bills", "balances", "budgets", "debts"];

pub struct Butler {
    pub window: gtk::ApplicationWindow,
    status: gtk::Label,
    approvals: gtk::Box,
    memory: gtk::Box,
    today: gtk::Label,
    pick: gtk::DropDown,
    turns: gtk::Box,
    linked: gtk::Label,
}

fn page(stack: &gtk::Stack, name: &str, title: &str, child: &impl IsA<gtk::Widget>) {
    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    scroll.set_child(Some(child));
    stack.add_titled(&scroll, Some(name), title);
}

fn column() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("page");
    b
}

fn text(s: &str) -> gtk::Label {
    gtk::Label::builder().label(s).wrap(true).xalign(0.0).selectable(true).hexpand(true).build()
}

fn clear(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

fn row(label: &gtk::Label, buttons: &[gtk::Button]) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    r.add_css_class("row");
    r.append(label);
    for b in buttons {
        b.set_valign(gtk::Align::Center);
        r.append(b);
    }
    r
}

impl Butler {
    pub fn new(app: &gtk::Application) -> Rc<Butler> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Butler").default_width(780).default_height(560).build();
        window.add_css_class("butler");
        let (header, refresh) = (gtk::HeaderBar::new(), gtk::Button::from_icon_name("view-refresh-symbolic"));
        refresh.set_tooltip_text(Some("Refresh"));
        header.pack_start(&refresh);
        window.set_titlebar(Some(&header));
        let stack = gtk::Stack::new();
        let pick = gtk::DropDown::from_strings(&TODAY);
        let today = text("");
        let today_page = column();
        today_page.append(&pick);
        today_page.append(&today);
        let me = Rc::new(Butler {
            window,
            status: gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build(),
            approvals: column(),
            memory: column(),
            today,
            pick,
            turns: column(),
            linked: text(""),
        });
        me.status.add_css_class("dim");
        header.pack_start(&me.status);
        let linked = column();
        linked.append(&me.linked);
        page(&stack, "approvals", "Approvals", &me.approvals);
        page(&stack, "memory", "Memory", &me.memory);
        page(&stack, "today", "Today", &today_page);
        page(&stack, "turns", "Turns", &me.turns);
        page(&stack, "linked", "Linked", &linked);
        stack.set_visible_child_name("approvals");
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&gtk::StackSidebar::builder().stack(&stack).build());
        body.append(&stack);
        stack.set_hexpand(true);
        me.window.set_child(Some(&body));
        let again = Rc::downgrade(&me);
        refresh.connect_clicked(move |_| {
            if let Some(me) = again.upgrade() {
                me.refresh();
            }
        });
        let again = Rc::downgrade(&me);
        me.pick.connect_selected_notify(move |_| {
            if let Some(me) = again.upgrade() {
                me.refresh();
            }
        });
        me.refresh();
        me
    }

    fn chosen(&self) -> &'static str {
        TODAY.get(self.pick.selected() as usize).copied().unwrap_or(TODAY[0])
    }

    /// Sends `requests` off the main thread, then hands each answer to `done`; trouble reaching Alfred shows in the
    /// header and leaves the pages as they were.
    fn ask(self: &Rc<Self>, requests: Vec<Value>, done: impl FnOnce(&Rc<Self>, Vec<Result<Value, String>>) + 'static) {
        self.status.set_label("Asking Alfred…");
        let me = Rc::clone(self);
        glib::spawn_future_local(async move {
            let asked = gio::spawn_blocking(move || {
                let config: door::Config = autark_common::config::load("butler");
                Door::new(&config)?.ask(&requests)
            })
            .await
            .unwrap_or_else(|_| Err("Asking Alfred stopped unexpectedly.".into()));
            match asked {
                Ok(answers) => {
                    me.status.set_label("");
                    done(&me, answers);
                }
                Err(e) => me.status.set_label(&e),
            }
        });
    }

    fn refresh(self: &Rc<Self>) {
        let requests = vec![
            json!({"op": "approvals"}),
            json!({"op": "memory"}),
            json!({"op": "today", "what": self.chosen()}),
            json!({"op": "turns", "n": 50}),
            json!({"op": "linked"}),
        ];
        self.ask(requests, |me, a| {
            me.show_approvals(&a[0]);
            me.show_memory(&a[1]);
            me.today.set_label(&said(&a[2]));
            me.show_turns(&a[3]);
            me.linked.set_label(&said(&a[4]));
        });
    }

    /// One change (a yes, a no, a forget): Alfred's answer shows in the header, then everything is fetched again.
    fn change(self: &Rc<Self>, request: Value) {
        self.ask(vec![request], |me, a| {
            me.refresh();
            me.status.set_label(&said(&a[0]));
        });
    }

    fn show_approvals(self: &Rc<Self>, answer: &Result<Value, String>) {
        clear(&self.approvals);
        let open = answer.as_ref().ok().and_then(Value::as_array).cloned().unwrap_or_default();
        if open.is_empty() {
            self.approvals.append(&text(&empty(answer, "Nothing is waiting for a yes.")));
        }
        for a in open {
            let id = a["id"].as_i64().unwrap_or(0);
            let (yes, no) = (gtk::Button::with_label("Yes"), gtk::Button::with_label("No"));
            yes.add_css_class("suggested-action");
            for (button, answer) in [(&yes, true), (&no, false)] {
                let me = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(me) = me.upgrade() {
                        me.change(json!({"op": "decide", "id": id, "yes": answer}));
                    }
                });
            }
            self.approvals.append(&row(&text(a["question"].as_str().unwrap_or("")), &[yes, no]));
        }
    }

    fn show_memory(self: &Rc<Self>, answer: &Result<Value, String>) {
        clear(&self.memory);
        let all = said(answer);
        let facts: Vec<&str> = all.lines().filter_map(|l| l.strip_prefix("- ")).collect();
        if facts.is_empty() {
            self.memory.append(&text(if answer.is_ok() { "Alfred doesn't remember anything yet." } else { &all }));
        }
        for fact in facts {
            let (forget, label) = (gtk::Button::with_label("Forget"), text(fact));
            let (me, fact) = (Rc::downgrade(self), fact.to_owned());
            forget.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.confirm_forget(fact.clone());
                }
            });
            self.memory.append(&row(&label, &[forget]));
        }
    }

    /// Forgetting can't be undone from here, so it asks first.
    fn confirm_forget(self: &Rc<Self>, fact: String) {
        let dialog = gtk::AlertDialog::builder()
            .message("Forget this?")
            .detail(fact.as_str())
            .buttons(["Cancel", "Forget"])
            .cancel_button(0)
            .default_button(0)
            .build();
        let me = Rc::clone(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            if choice == Ok(1) {
                me.change(json!({"op": "forget", "fact": fact}));
            }
        });
    }

    fn show_turns(&self, answer: &Result<Value, String>) {
        clear(&self.turns);
        let turns = answer.as_ref().ok().and_then(Value::as_array).cloned().unwrap_or_default();
        if turns.is_empty() {
            self.turns.append(&text(&empty(answer, "No turns yet.")));
        }
        for t in turns {
            let when = t["at"].as_i64().and_then(|at| glib::DateTime::from_unix_local(at).ok());
            let when = when.and_then(|w| w.format("%a %H:%M").ok()).map(|w| w.to_string()).unwrap_or_default();
            let label = text(&format!("{when}  {}", door::turn(&t)));
            self.turns.append(&row(&label, &[]));
        }
    }
}

/// What an empty page says: `none`, or why Alfred's answer didn't come.
fn empty(answer: &Result<Value, String>, none: &str) -> String {
    if answer.is_ok() { none.to_owned() } else { said(answer) }
}

/// An answer as text: Alfred's words, or why there are none.
fn said(answer: &Result<Value, String>) -> String {
    match answer {
        Ok(Value::String(s)) => s.clone(),
        Ok(other) => other.to_string(),
        Err(e) => e.clone(),
    }
}
