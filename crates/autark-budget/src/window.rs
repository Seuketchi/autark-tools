//! The window: a sidebar of panes (your paychecks, spending, plan and electricity) and the pane beside it.

use std::rc::Rc;

use autark_finance::store::Store;
use gtk::glib;
use gtk::prelude::*;

use crate::calendar_pane::CalendarPane;
use crate::electric_pane::ElectricPane;
use crate::paychecks_pane::PaychecksPane;
use crate::plan_pane::PlanPane;
use crate::settings::Settings;
use crate::spending_pane::SpendingPane;

/// Shows the pane of that name by selecting its row in the sidebar.
fn select_pane(sidebar: &gtk::ListBox, name: &str) {
    let mut at = 0;
    while let Some(row) = sidebar.row_at_index(at) {
        if row.widget_name() == name {
            sidebar.select_row(Some(&row));
            return;
        }
        at += 1;
    }
}

pub struct Budget {
    pub window: gtk::ApplicationWindow,
    /// The buttons hold only weak references to their pane, so the window has to keep it alive.
    _electric: Rc<ElectricPane>,
    _paychecks: Rc<PaychecksPane>,
    _calendar: Rc<CalendarPane>,
    _spending: Rc<SpendingPane>,
    _plan: Rc<PlanPane>,
}

impl Budget {
    pub fn new(app: &gtk::Application, store: Store, settings: Settings) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Budget").default_width(940).default_height(700).build();
        window.add_css_class("budget");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let handle = gtk::WindowHandle::builder().child(&autark_common::chrome::controls(true)).build();
        handle.add_css_class("handle");

        // Each pane is a name for the sidebar and a widget for the stack. A new tracker is one more line here.
        let settings = Rc::new(std::cell::RefCell::new(settings));
        let electric = ElectricPane::new(store.clone(), settings.clone());
        let paychecks = PaychecksPane::new(store.clone(), settings.clone());
        let calendar = CalendarPane::new(store.clone(), settings.clone());
        let spending = SpendingPane::new(store.clone(), settings.clone());
        let plan = PlanPane::new(store, settings);
        let panes: [(&str, &str, gtk::Widget); 5] = [
            ("paychecks", "Paychecks", paychecks.root.clone().upcast()),
            ("calendar", "Calendar", calendar.root.clone().upcast()),
            ("spending", "Spending", spending.root.clone().upcast()),
            ("plan", "Plan", plan.root.clone().upcast()),
            ("electric", "Electricity", electric.root.clone().upcast()),
        ];

        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        let sidebar = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).build();
        sidebar.add_css_class("sidebar-list");
        for (name, title, widget) in &panes {
            stack.add_named(widget, Some(name));
            let label = gtk::Label::builder().label(*title).xalign(0.0).build();
            label.add_css_class("pane-name");
            let row = gtk::ListBoxRow::builder().child(&label).build();
            row.set_widget_name(name);
            sidebar.append(&row);
        }
        // Another pane may have changed what this one shows, so a pane redraws as it is opened.
        sidebar.connect_row_selected({
            let stack = stack.clone();
            let (paychecks, calendar, spending, plan, electric) = (Rc::downgrade(&paychecks), Rc::downgrade(&calendar), Rc::downgrade(&spending), Rc::downgrade(&plan), Rc::downgrade(&electric));
            move |_, row| {
                let Some(row) = row else { return };
                match row.widget_name().as_str() {
                    "paychecks" => paychecks.upgrade().iter().for_each(|p| p.refresh()),
                    "calendar" => calendar.upgrade().iter().for_each(|p| p.show_today()),
                    "spending" => spending.upgrade().iter().for_each(|p| p.refresh()),
                    "plan" => plan.upgrade().iter().for_each(|p| p.refresh()),
                    "electric" => electric.upgrade().iter().for_each(|p| p.refresh()),
                    _ => {}
                }
                stack.set_visible_child_name(&row.widget_name());
            }
        });
        if let Some(first) = sidebar.row_at_index(0) {
            sidebar.select_row(Some(&first));
        }

        // Edit on a payday's line opens that item in Plan.
        paychecks.on_edit({
            let (sidebar, plan) = (sidebar.downgrade(), Rc::downgrade(&plan));
            move |name| {
                let (Some(sidebar), Some(plan)) = (sidebar.upgrade(), plan.upgrade()) else { return };
                select_pane(&sidebar, "plan");
                plan.edit_named(name);
            }
        });

        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("side");
        side.set_width_request(190);
        side.append(&sidebar);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&side);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&stack);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&body);
        window.set_child(Some(&content));

        let budget = Rc::new(Self { window, _electric: electric, _paychecks: paychecks, _calendar: calendar, _spending: spending, _plan: plan });
        let keep = budget.clone();
        budget.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });
        budget
    }
}

#[cfg(test)]
mod tests {
    use autark_finance::plan::{Every, Item};
    use gtk::gio;

    use super::*;

    /// Every widget under `root`, depth first.
    fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut found = vec![root.clone()];
        let mut child = root.first_child();
        while let Some(widget) = child {
            found.extend(descendants(&widget));
            child = widget.next_sibling();
        }
        found
    }

    /// Needs a display and the main thread, so it only runs on request:
    /// `cargo test -p autark-budget -- --ignored --test-threads=1`.
    ///
    /// It once failed because the window let go of its pane, so every button did nothing and no reading was saved.
    #[test]
    #[ignore = "needs a display"]
    fn adding_a_reading_in_the_window_saves_it() {
        let dir = std::env::temp_dir().join(format!("autark-budget-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        gtk::init().expect("a display");
        let app = gtk::Application::builder().application_id("org.autark.BudgetTest").flags(gio::ApplicationFlags::NON_UNIQUE).build();
        app.register(gio::Cancellable::NONE).unwrap();

        // Nothing but the window holds the pane, as in the real app.
        let budget = Budget::new(&app, Store::open(&dir), Settings::default());
        let widgets = descendants(budget.window.upcast_ref());
        let entry = |placeholder: &str| {
            widgets.iter().find_map(|w| w.downcast_ref::<gtk::Entry>().filter(|e| e.placeholder_text().as_deref() == Some(placeholder))).unwrap().clone()
        };
        entry("Meter reading, kWh").set_text("15.4");
        entry("Date (today)").set_text("2026-11-01");
        let add = widgets.iter().find_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some("Add"))).unwrap();
        add.emit_clicked();

        let saved = Store::open(&dir).readings();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved.len(), 1, "pressing Add saved nothing");
        assert_eq!(saved[0].kwh, 15.4);
    }

    /// Everything of one kind under the window, looked up afresh because Edit adds widgets.
    fn buttons(window: &gtk::ApplicationWindow, label: &str) -> Vec<gtk::Button> {
        descendants(window.upcast_ref()).iter().filter_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some(label)).cloned()).collect()
    }

    fn entry_named(window: &gtk::ApplicationWindow, placeholder: &str) -> gtk::Entry {
        descendants(window.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Entry>().filter(|e| e.placeholder_text().as_deref() == Some(placeholder))).unwrap().clone()
    }

    fn open_window(dir: &std::path::Path, id: &str, readings: &[(chrono::NaiveDate, f64)]) -> Rc<Budget> {
        use autark_finance::electric::Reading;
        let _ = std::fs::remove_dir_all(dir);
        let store = Store::open(dir);
        for (date, kwh) in readings {
            store.add_reading(&Reading { date: *date, kwh: *kwh, note: String::new() }).unwrap();
        }
        gtk::init().expect("a display");
        let app = gtk::Application::builder().application_id(id).flags(gio::ApplicationFlags::NON_UNIQUE).build();
        app.register(gio::Cancellable::NONE).unwrap();
        Budget::new(&app, Store::open(dir), Settings::default())
    }

    /// Edit turns the reading's own row into a form; saving writes over that reading and leaves the others alone.
    #[test]
    #[ignore = "needs a display"]
    fn editing_a_reading_changes_it_in_place() {
        let dir = std::env::temp_dir().join(format!("autark-budget-edit-{}", std::process::id()));
        // All in one month, so they are all in the open month of the history.
        let june = |day| chrono::NaiveDate::from_ymd_opt(2026, 6, day).unwrap();
        let budget = open_window(&dir, "org.autark.BudgetEdit", &[(june(10), 10.0), (june(11), 12.0), (june(12), 14.0)]);
        let window = &budget.window;

        // The history is newest first, so the second Edit is the reading of 12.
        let edits = buttons(window, "Edit");
        assert_eq!(edits.len(), 3, "one Edit for each reading");
        edits[1].emit_clicked();
        assert_eq!(entry_named(window, "Meter, kWh").text(), "12", "Edit should open that reading's own row for editing");
        assert_eq!(entry_named(window, "Date").text(), "2026-06-11");

        entry_named(window, "Meter, kWh").set_text("12.5");
        buttons(window, "Save reading")[0].emit_clicked();
        let saved: Vec<f64> = Store::open(&dir).readings().iter().map(|r| r.kwh).collect();
        assert_eq!(saved, [10.0, 12.5, 14.0], "saving should replace that reading and keep the others");
        assert!(buttons(window, "Save reading").is_empty(), "the row goes back to normal once saved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reading from today, saved with Enter, and a nonsense number refused with the row left open to fix.
    #[test]
    #[ignore = "needs a display"]
    fn editing_the_only_reading_from_today_with_enter() {
        let dir = std::env::temp_dir().join(format!("autark-budget-edit1-{}", std::process::id()));
        let today = chrono::Local::now().date_naive();
        let budget = open_window(&dir, "org.autark.BudgetEdit1", &[(today, 16.6)]);
        let window = &budget.window;
        buttons(window, "Edit")[0].emit_clicked();
        assert_eq!(entry_named(window, "Meter, kWh").text(), "16.6");
        assert_eq!(entry_named(window, "Date").text(), today.to_string());

        entry_named(window, "Meter, kWh").set_text("abc");
        entry_named(window, "Meter, kWh").emit_activate();
        assert_eq!(Store::open(&dir).readings()[0].kwh, 16.6);
        assert_eq!(buttons(window, "Save reading").len(), 1, "the form stays open after a mistake");

        entry_named(window, "Meter, kWh").set_text("15.4");
        entry_named(window, "Meter, kWh").emit_activate();
        let saved: Vec<f64> = Store::open(&dir).readings().iter().map(|r| r.kwh).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, [15.4]);
    }

    /// Cancel puts the row back as it was.
    #[test]
    #[ignore = "needs a display"]
    fn cancelling_an_edit_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("autark-budget-edit2-{}", std::process::id()));
        let budget = open_window(&dir, "org.autark.BudgetEdit2", &[(chrono::NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(), 10.0)]);
        let window = &budget.window;
        buttons(window, "Edit")[0].emit_clicked();
        entry_named(window, "Meter, kWh").set_text("99");
        buttons(window, "Cancel")[0].emit_clicked();
        assert_eq!(Store::open(&dir).readings()[0].kwh, 10.0);
        assert!(buttons(window, "Save reading").is_empty());
        assert_eq!(buttons(window, "Edit").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The money panes save to the settings file, so a test that drives them must never see the real one.
    fn isolated(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("autark-budget-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // One window test per process, so nothing else is reading the environment.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
            // Backups go under the home folder.
            std::env::set_var("HOME", &dir);
        }
        dir
    }

    fn open_money(dir: &std::path::Path, id: &str, settings: Settings) -> Rc<Budget> {
        gtk::init().expect("a display");
        let app = gtk::Application::builder().application_id(id).flags(gio::ApplicationFlags::NON_UNIQUE).build();
        app.register(gio::Cancellable::NONE).unwrap();
        Budget::new(&app, Store::open(dir.join("data")), settings)
    }

    fn entry_in(root: &impl IsA<gtk::Widget>, placeholder: &str) -> gtk::Entry {
        descendants(root.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Entry>().filter(|e| e.placeholder_text().as_deref() == Some(placeholder))).unwrap().clone()
    }

    fn money_settings() -> Settings {
        use autark_finance::money::Money;
        let mut settings = Settings::default();
        settings.finance.paycheck = Money::from_pesos(10_000);
        settings.finance.items.push(Item { name: "Laundry".into(), amount: Money::from_pesos(170), every: Every::Payday, ..Item::default() });
        settings
    }

    /// Ticking a bill on the Paychecks pane records the payment; unticking takes it back.
    #[test]
    #[ignore = "needs a display"]
    fn ticking_a_bill_records_the_payment() {
        let dir = isolated("tick");
        let budget = open_money(&dir, "org.autark.BudgetTick", money_settings());
        let checks = || descendants(budget._paychecks.root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::CheckButton>().ok()).collect::<Vec<_>>();
        // Laundry is due every payday, and four paydays are shown; the first card is this payday's.
        assert!(checks().len() >= 4, "a tick for every line of every payday shown");
        let first = checks().remove(0);
        first.set_active(true);
        let paid = Store::open(dir.join("data")).payments();
        assert_eq!(paid.len(), 1, "ticking saved nothing");
        assert_eq!(paid[0].item, "Laundry");
        assert_eq!(paid[0].amount, autark_finance::money::Money::from_pesos(170));
        first.set_active(false);
        let left = Store::open(dir.join("data")).payments();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(left.is_empty(), "unticking should take the payment back");
    }

    /// Edit on a line of the Paychecks pane opens that item in Plan, and saving there changes the amount in place.
    #[test]
    #[ignore = "needs a display"]
    fn editing_a_one_time_item_from_its_paycheck_line() {
        use autark_finance::money::Money;
        let dir = isolated("edit-item");
        let mut settings = money_settings();
        let today = chrono::Local::now().date_naive();
        settings.finance.items.push(Item { name: "Deposit".into(), amount: Money::from_pesos(2_500), every: Every::Once, date: Some(today), ..Item::default() });
        let budget = open_money(&dir, "org.autark.BudgetEditItem", settings);

        let edit = descendants(budget._paychecks.root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).filter(|b| b.label().as_deref() == Some("Edit")).collect::<Vec<_>>();
        assert!(edit.len() >= 2, "every line of the plan has an Edit");
        edit[0].emit_clicked();
        let pane = &budget._plan.root;
        let name = entry_in(pane, "e.g. Rent").text();
        assert!(!name.is_empty(), "Edit should fill the form");
        entry_in(pane, "e.g. 4000").set_text("3000");
        descendants(pane.upcast_ref()).iter().filter_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some("Save")).cloned()).next().unwrap().emit_clicked();
        let items = Settings::load().finance.items;
        let _ = std::fs::remove_dir_all(&dir);
        let changed = items.iter().find(|i| i.name == name.as_str()).unwrap();
        assert_eq!(changed.amount, Money::from_pesos(3_000));
        assert_eq!(items.len(), 2, "replaced in place, not added");
    }

    /// Lets the idle redraws the panes ask for happen.
    fn settle() {
        let context = gtk::glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
    }

    /// A paid line shows what was paid as text. Only Edit, then Save or Enter, alters it; a bad value is refused
    /// with the box left open, and Cancel changes nothing.
    #[test]
    #[ignore = "needs a display"]
    fn changing_what_you_paid_changes_the_payment() {
        use autark_finance::money::Money;
        let dir = isolated("paid-amount");
        let budget = open_money(&dir, "org.autark.BudgetPaidAmount", money_settings());
        let root = budget._paychecks.root.clone();
        descendants(root.upcast_ref()).into_iter().find_map(|w| w.downcast::<gtk::CheckButton>().ok()).unwrap().set_active(true);
        settle();
        let widgets = || descendants(root.upcast_ref());
        let button = |label: &str| widgets().into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).find(|b| b.label().as_deref() == Some(label)).unwrap();
        let paid_box = || widgets().into_iter().find_map(|w| w.downcast::<gtk::Entry>().ok()).expect("the box exists but is hidden");
        let paid = || Store::open(dir.join("data")).payments()[0].amount;

        assert!(!paid_box().get_visible(), "the amount is plain text until Edit is pressed");
        // Typing into the hidden box and pressing Enter does nothing to the payment.
        paid_box().set_text("999");
        paid_box().emit_activate();
        settle();
        assert_eq!(paid(), Money::from_pesos(170));

        button("Edit").emit_clicked();
        assert!(paid_box().get_visible());
        assert_eq!(paid_box().text(), "170");
        paid_box().set_text("abc");
        button("Save").emit_clicked();
        settle();
        assert_eq!(paid(), Money::from_pesos(170), "a bad amount changes nothing");
        assert!(paid_box().get_visible(), "and the box stays open to fix");

        paid_box().set_text("999");
        button("Cancel").emit_clicked();
        assert!(!paid_box().get_visible());
        assert_eq!(paid(), Money::from_pesos(170), "Cancel changes nothing");

        button("Edit").emit_clicked();
        paid_box().set_text("150.50");
        button("Save").emit_clicked();
        settle();
        let now = paid();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(now, Money::from_centavos(15_050));
    }

    /// Delete on the Plan pane can be taken back, and the item returns to the same place.
    #[test]
    #[ignore = "needs a display"]
    fn a_deleted_plan_item_can_be_put_back() {
        use autark_finance::money::Money;
        let dir = isolated("undo");
        let mut settings = money_settings();
        settings.finance.items.push(Item { name: "Water".into(), amount: Money::from_pesos(300), every: Every::Month, day: Some(5), ..Item::default() });
        let budget = open_money(&dir, "org.autark.BudgetUndo", settings);
        let pane = budget._plan.root.clone();
        let button = |label: &str| descendants(pane.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).filter(|b| b.label().as_deref() == Some(label)).collect::<Vec<_>>();
        assert!(!button("Undo").iter().any(|b| b.get_visible()), "nothing to undo yet");

        // Bills come first on the page, in the order of the plan: Laundry, then Water.
        button("Delete")[0].emit_clicked();
        settle();
        let names = |items: Vec<Item>| items.into_iter().map(|i| i.name).collect::<Vec<_>>();
        assert_eq!(names(Settings::load().finance.items), ["Water"]);
        assert!(button("Undo").iter().any(|b| b.get_visible()), "Undo appears after a delete");

        button("Undo").into_iter().find(|b| b.get_visible()).unwrap().emit_clicked();
        settle();
        let back = names(Settings::load().finance.items);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, ["Laundry", "Water"], "put back where it was");
    }

    /// Spending categories are set on the Plan pane and become the Spending pane's buttons.
    #[test]
    #[ignore = "needs a display"]
    fn the_spending_buttons_follow_the_plans_categories() {
        let dir = isolated("categories");
        let budget = open_money(&dir, "org.autark.BudgetCategories", money_settings());
        let labels = |root: &gtk::ScrolledWindow| descendants(root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).filter_map(|b| b.label().map(|l| l.to_string())).filter(|l| l != "Check").collect::<Vec<_>>();
        assert!(labels(&budget._spending.root).starts_with(&["Food".to_string(), "Transport".to_string(), "Load".to_string(), "Other".to_string()]));

        entry_in(&budget._plan.root, "e.g. Food, Transport, Load").set_text("Food, Pets");
        entry_in(&budget._plan.root, "e.g. Food, Transport, Load").emit_activate();
        settle();
        budget._spending.refresh();
        let spending = labels(&budget._spending.root);
        let saved = Settings::load().finance.categories;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, ["Food", "Pets"]);
        assert_eq!(&spending[..3], ["Food", "Pets", "Other"]);
    }

    /// Back up now copies the settings and logs into the dated folder under the home folder, here a temporary one.
    #[test]
    #[ignore = "needs a display"]
    fn backing_up_from_the_plan_pane_copies_the_logs() {
        let dir = isolated("backup");
        let budget = open_money(&dir, "org.autark.BudgetBackup", money_settings());
        Store::open(dir.join("data")).add_spending(&autark_finance::spending::Spend { date: crate::view::today(), amount: autark_finance::money::Money::from_pesos(50), category: String::new(), note: String::new() }).unwrap();
        descendants(budget._plan.root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).find(|b| b.label().as_deref() == Some("Back up now")).unwrap().emit_clicked();
        let folder = dir.join("Backups").join("autark-budget").join(crate::view::today().to_string());
        let copied = folder.join("spending.jsonl").is_file();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(copied, "the backup folder should hold the spending log");
    }

    /// The calendar moves between months with the arrows, Today brings it back, and a day opens its details.
    #[test]
    #[ignore = "needs a display"]
    fn the_calendar_moves_between_months_and_opens_a_day() {
        use chrono::Datelike;
        let dir = isolated("calendar");
        let budget = open_money(&dir, "org.autark.BudgetCalendar", money_settings());
        let root = budget._calendar.root.clone();
        let title = || descendants(root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Label>().ok()).find(|l| l.has_css_class("projected")).unwrap().label().to_string();
        let button = |label: &str| descendants(root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).find(|b| b.label().as_deref() == Some(label)).unwrap();
        let today = crate::view::today();
        assert_eq!(title(), today.format("%B %Y").to_string());

        button("›").emit_clicked();
        settle();
        let (year, month) = autark_finance::calendar::shift(today.year(), today.month(), 1);
        assert_eq!(title(), chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap().format("%B %Y").to_string());
        button("‹").emit_clicked();
        button("‹").emit_clicked();
        settle();
        assert_ne!(title(), today.format("%B %Y").to_string());
        button("Today").emit_clicked();
        settle();
        assert_eq!(title(), today.format("%B %Y").to_string());

        // Picking another day shows that day under the grid.
        let cells = || descendants(root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).filter(|b| b.has_css_class("cal-day")).collect::<Vec<_>>();
        assert!(cells().len() >= 28 && cells().len() % 7 == 0, "whole weeks of days");
        let other = cells().into_iter().find(|b| b.has_css_class("selected")).is_some();
        assert!(other, "today starts picked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The price box answers yes or no, and a bad price is refused.
    #[test]
    #[ignore = "needs a display"]
    fn the_price_box_says_whether_it_fits() {
        let dir = isolated("afford");
        let budget = open_money(&dir, "org.autark.BudgetAfford", money_settings());
        let pane = &budget._spending.root;
        let answer = || descendants(pane.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Label>().ok()).filter(|l| l.has_css_class("verdict")).map(|l| l.label().to_string()).collect::<Vec<_>>().join(" | ");
        let check = || descendants(pane.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Button>().ok()).find(|b| b.label().as_deref() == Some("Check")).unwrap().emit_clicked();

        entry_in(pane, "Price, ₱").set_text("100");
        check();
        assert!(answer().contains("Yes. ₱100 leaves"), "{}", answer());
        entry_in(pane, "Price, ₱").set_text("9999999");
        check();
        assert!(answer().contains("Not this payday"), "{}", answer());
        entry_in(pane, "Price, ₱").set_text("abc");
        check();
        let said = answer();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said.contains("is not a price"), "{said}");
    }

    /// Typing an amount and tapping a category logs it; a bad amount is refused and logs nothing.
    #[test]
    #[ignore = "needs a display"]
    fn logging_a_purchase_saves_it() {
        let dir = isolated("spend");
        let budget = open_money(&dir, "org.autark.BudgetSpend", money_settings());
        let pane = &budget._spending.root;
        entry_in(pane, "Amount, ₱").set_text("abc");
        descendants(pane.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some("Food"))).unwrap().emit_clicked();
        assert!(Store::open(dir.join("data")).spending().is_empty(), "a bad amount must log nothing");

        entry_in(pane, "Amount, ₱").set_text("120.50");
        entry_in(pane, "What for (optional)").set_text("lunch");
        descendants(pane.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some("Food"))).unwrap().emit_clicked();
        let spent = Store::open(dir.join("data")).spending();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(spent.len(), 1);
        assert_eq!((spent[0].category.as_str(), spent[0].note.as_str()), ("Food", "lunch"));
        assert_eq!(spent[0].amount, autark_finance::money::Money::from_centavos(12_050));
    }

    /// Adding an item on the Plan pane lands in the settings file; a bad one is refused with a reason.
    #[test]
    #[ignore = "needs a display"]
    fn adding_an_item_in_the_plan_saves_it() {
        let dir = isolated("plan");
        let budget = open_money(&dir, "org.autark.BudgetPlan", Settings::default());
        let pane = &budget._plan.root;
        let save = || descendants(pane.upcast_ref()).iter().filter_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some("Save")).cloned()).next().unwrap().emit_clicked();

        let button = |label: &str| descendants(pane.upcast_ref()).iter().find_map(|w| w.downcast_ref::<gtk::Button>().filter(|b| b.label().as_deref() == Some(label)).cloned()).unwrap();
        button("Add something").emit_clicked();
        entry_in(pane, "e.g. Rent").set_text("Rent");
        save();
        assert!(Settings::load().finance.items.is_empty(), "no amount, so nothing is saved");

        entry_in(pane, "e.g. 4000").set_text("4000");
        entry_in(pane, "1 to 31").set_text("31");
        save();
        let items = Settings::load().finance.items;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].name.as_str(), items[0].every, items[0].day), ("Rent", Every::Month, Some(31)));
    }

    /// `PANE_PNG=/some/folder cargo test -p autark-budget dump_the_pane -- --ignored --test-threads=1` draws every tab of
    /// the electricity pane, with four months of made-up readings in a folder of its own, to look at.
    #[test]
    #[ignore = "needs a display; writes pictures when PANE_PNG is set"]
    fn dump_the_pane() {
        use autark_finance::electric::{Bill, Reading};
        use autark_finance::money::Money;
        let Some(folder) = std::env::var_os("PANE_PNG") else { return };
        let dir = std::env::temp_dir().join(format!("autark-budget-pane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir);
        let today = chrono::Local::now().date_naive();
        let mut meter = 15.4;
        for back in (0..=120).rev() {
            let date = today - chrono::Duration::days(back);
            let weekend = matches!(chrono::Datelike::weekday(&date), chrono::Weekday::Sat | chrono::Weekday::Sun);
            meter += if weekend { 4.6 } else { 2.1 + (back % 3) as f64 * 0.3 };
            store.add_reading(&Reading { date, kwh: meter, note: if back == 40 { "new fridge".into() } else { String::new() } }).unwrap();
        }
        for (from, to, kwh, pesos) in [(95, 66, 76.0, 912), (65, 36, 84.0, 1050)] {
            let bill = Bill { from: today - chrono::Duration::days(from), to: today - chrono::Duration::days(to), kwh, amount: Money::from_pesos(pesos) };
            store.add_bill(&bill).unwrap();
        }

        gtk::init().expect("a display");
        autark_common::theme::install(include_str!("budget.css"));
        let app = gtk::Application::builder().application_id("org.autark.BudgetPane").flags(gio::ApplicationFlags::NON_UNIQUE).build();
        app.register(gio::Cancellable::NONE).unwrap();
        let mut settings = Settings::default();
        // A cycle that began a couple of weeks ago, so there is something to project.
        settings.electric.billing_day = 15;
        let budget = Budget::new(&app, store, settings);
        // GTK draws nothing for a window that was never shown, so show it for a moment and let it lay itself out.
        budget.window.present();
        let context = gtk::glib::MainContext::default();
        let pump = |millis| {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(millis);
            while std::time::Instant::now() < until {
                context.iteration(false);
            }
        };
        pump(500);

        let everything = descendants(budget.window.upcast_ref());
        let tabs = everything.iter().find(|w| w.widget_name() == "electric-tabs").unwrap().downcast_ref::<gtk::Stack>().unwrap().clone();
        for name in ["overview", "trends", "log", "bills", "settings"] {
            tabs.set_visible_child_name(name);
            if name == "log" {
                // Show a row being edited, with a refused value so the reason shows too.
                buttons(&budget.window, "Edit")[0].emit_clicked();
                entry_named(&budget.window, "Meter, kWh").set_text("1");
                buttons(&budget.window, "Save reading")[0].emit_clicked();
            }
            pump(250);
            let page = descendants(tabs.visible_child().unwrap().upcast_ref()).into_iter().find(|w| w.has_css_class("page")).unwrap();
            let width = 760;
            let (_, height, _, _) = page.measure(gtk::Orientation::Vertical, width);
            // The page can be taller than the window; lay all of it out for the picture.
            page.allocate(width, height, -1, None);
            let snapshot = gtk::Snapshot::new();
            page.parent().unwrap().snapshot_child(&page, &snapshot);
            let node = snapshot.to_node().expect("something drawn");
            let renderer = gtk::gsk::CairoRenderer::new();
            renderer.realize_for_display(&gtk::gdk::Display::default().unwrap()).unwrap();
            renderer.render_texture(&node, None).save_to_png(std::path::Path::new(&folder).join(format!("{name}.png"))).unwrap();
        }
        budget.window.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `PANE_PNG=/some/folder` draws the Paychecks, Spending and Plan panes, with a made-up plan and a few made-up
    /// purchases in a folder of their own, to look at.
    #[test]
    #[ignore = "needs a display; writes pictures when PANE_PNG is set"]
    fn dump_the_money_panes() {
        use autark_finance::money::Money;
        use autark_finance::spending::Spend;
        let Some(folder) = std::env::var_os("PANE_PNG") else { return };
        let dir = isolated("money-pictures");
        let store = Store::open(dir.join("data"));
        let today = chrono::Local::now().date_naive();
        let mut settings = Settings::default();
        let config = &mut settings.finance;
        config.paycheck = Money::from_centavos(1_000_000);
        let bill = |name: &str, kind, pesos: i64, every, day, target: Option<i64>, saved: i64| Item {
            name: name.into(),
            kind,
            amount: Money::from_pesos(pesos),
            every,
            day,
            target: target.map(Money::from_pesos),
            saved: Money::from_pesos(saved),
            ..Item::default()
        };
        use autark_finance::plan::Kind;
        config.items = vec![
            bill("Rent", Kind::Bill, 3_500, Every::Month, Some(31), None, 0),
            bill("Internet", Kind::Bill, 1_200, Every::Month, Some(15), None, 0),
            bill("Laundry", Kind::Bill, 150, Every::Payday, None, None, 0),
            bill("Bike fund", Kind::Goal, 4_000, Every::Payday, None, Some(20_000), 8_000),
            Item { times: Some(3), date: Some(today), ..bill("Phone plan", Kind::Installment, 400, Every::Month, Some(20), None, 0) },
        ];
        for (back, amount, category, note) in [(0, 85, "Food", "lunch"), (0, 20, "Transport", ""), (1, 240, "Food", "groceries"), (2, 100, "Load", ""), (20, 900, "Food", "last payday"), (40, 2_000, "Food", "")] {
            let spend = Spend { date: today - chrono::Duration::days(back), amount: Money::from_pesos(amount), category: category.into(), note: note.into() };
            store.add_spending(&spend).unwrap();
        }

        gtk::init().expect("a display");
        autark_common::theme::install(include_str!("budget.css"));
        let app = gtk::Application::builder().application_id("org.autark.BudgetMoneyPictures").flags(gio::ApplicationFlags::NON_UNIQUE).build();
        app.register(gio::Cancellable::NONE).unwrap();
        let budget = Budget::new(&app, store, settings);
        budget.window.present();
        let context = gtk::glib::MainContext::default();
        let pump = |millis| {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(millis);
            while std::time::Instant::now() < until {
                context.iteration(false);
            }
        };
        pump(500);

        let sidebar = descendants(budget.window.upcast_ref()).into_iter().find_map(|w| w.downcast::<gtk::ListBox>().ok()).unwrap();
        // Tick the first line of this payday so the picture shows a paid one.
        descendants(budget._paychecks.root.upcast_ref()).into_iter().find_map(|w| w.downcast::<gtk::CheckButton>().ok()).unwrap().set_active(true);
        pump(100);
        for expander in descendants(budget._paychecks.root.upcast_ref()).into_iter().filter_map(|w| w.downcast::<gtk::Expander>().ok()) {
            expander.set_expanded(true);
        }
        pump(100);
        for (index, name, pane, editing) in [
            ("paychecks", "paychecks", &budget._paychecks.root, None),
            ("calendar", "calendar", &budget._calendar.root, None),
            ("spending", "spending", &budget._spending.root, None),
            ("plan", "plan", &budget._plan.root, None),
            ("plan", "plan-edit", &budget._plan.root, Some("Bike fund")),
            ("plan", "plan-edit-installment", &budget._plan.root, Some("Phone plan")),
        ] {
            select_pane(&sidebar, index);
            if let Some(item) = editing {
                budget._plan.edit_named(item);
            }
            pump(250);
            let page = descendants(pane.upcast_ref()).into_iter().find(|w| w.has_css_class("page")).unwrap();
            let width = 760;
            let (_, height, _, _) = page.measure(gtk::Orientation::Vertical, width);
            page.allocate(width, height, -1, None);
            let snapshot = gtk::Snapshot::new();
            page.parent().unwrap().snapshot_child(&page, &snapshot);
            let node = snapshot.to_node().expect("something drawn");
            let renderer = gtk::gsk::CairoRenderer::new();
            renderer.realize_for_display(&gtk::gdk::Display::default().unwrap()).unwrap();
            renderer.render_texture(&node, None).save_to_png(std::path::Path::new(&folder).join(format!("{name}.png"))).unwrap();
        }
        budget.window.close();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
