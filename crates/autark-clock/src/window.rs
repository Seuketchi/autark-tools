//! The clock's window: three pages, Alarm, Timer and Stopwatch. Alarms and the timer are handed to systemd to ring
//! (see `schedule`); the stopwatch only runs while the window is open.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::schedule;
use crate::state::{Alarm, DAYS, State, Timer, now};

/// `1:05` for 65 seconds, and `1:01:05` from an hour up.
pub fn countdown(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes:02}:{seconds:02}") }
}

/// `01:05.25`, and `1:01:05.25` from an hour up.
pub fn stopwatch(time: Duration) -> String {
    let hundredths = time.subsec_millis() / 10;
    format!("{}.{hundredths:02}", countdown(time.as_secs()))
}

/// How an alarm's days read: "Every day", "Weekdays", "Weekends", or the days themselves.
pub fn days_text(days: &[String]) -> String {
    let has = |wanted: &[&str]| days.len() == wanted.len() && wanted.iter().all(|day| days.iter().any(|d| d == day));
    if days.is_empty() {
        "Once".to_owned()
    } else if days.len() == 7 {
        "Every day".to_owned()
    } else if has(&["Mon", "Tue", "Wed", "Thu", "Fri"]) {
        "Weekdays".to_owned()
    } else if has(&["Sat", "Sun"]) {
        "Weekends".to_owned()
    } else {
        DAYS.iter().filter(|day| days.iter().any(|d| d == *day)).copied().collect::<Vec<_>>().join(", ")
    }
}

#[derive(Default)]
struct Watch {
    started: Option<Instant>,
    before: Duration,
    laps: Vec<Duration>,
}

impl Watch {
    fn elapsed(&self) -> Duration {
        self.before + self.started.map_or(Duration::ZERO, |started| started.elapsed())
    }
}

pub struct Clock {
    pub window: gtk::ApplicationWindow,
    exe: PathBuf,
    state: RefCell<State>,
    stack: gtk::Stack,
    // Alarm
    alarm_list: gtk::Box,
    form: gtk::Revealer,
    hour: gtk::SpinButton,
    minute: gtk::SpinButton,
    label: gtk::Entry,
    day_buttons: Vec<gtk::ToggleButton>,
    problem: gtk::Label,
    // Timer
    set_h: gtk::SpinButton,
    set_m: gtk::SpinButton,
    set_s: gtk::SpinButton,
    timer_setup: gtk::Box,
    timer_running: gtk::Box,
    timer_display: gtk::Label,
    start_timer: gtk::Button,
    // Stopwatch
    watch: RefCell<Watch>,
    watch_display: gtk::Label,
    watch_main: gtk::Button,
    watch_side: gtk::Button,
    laps: gtk::Box,
    /// Counts the ticks, so slower things happen only now and then.
    ticks: Cell<u32>,
}

fn two_digits(spin: &gtk::SpinButton) {
    spin.connect_output(|spin| {
        spin.set_text(&format!("{:02}", spin.value_as_int()));
        glib::Propagation::Stop
    });
}

fn spin(max: f64, value: f64) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(0.0, max, 1.0);
    spin.set_wrap(true);
    spin.set_orientation(gtk::Orientation::Vertical);
    spin.set_value(value);
    spin.set_numeric(true);
    spin.add_css_class("digits");
    two_digits(&spin);
    spin
}

fn button(label: &str, style: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("action");
    button.add_css_class(style);
    button
}

impl Clock {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Clock").default_width(440).default_height(620).build();
        window.add_css_class("clock");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("autark-clock"));

        // Pages and the tabs that choose between them.
        let stack = gtk::Stack::builder().vexpand(true).transition_type(gtk::StackTransitionType::Crossfade).build();
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("tabs");
        tabs.set_halign(gtk::Align::Center);
        let mut group: Option<gtk::ToggleButton> = None;
        for (name, title) in [("alarm", "Alarm"), ("timer", "Timer"), ("stopwatch", "Stopwatch")] {
            let tab = gtk::ToggleButton::with_label(title);
            tab.add_css_class("tab");
            tab.set_active(name == "alarm");
            if let Some(first) = &group {
                tab.set_group(Some(first));
            } else {
                group = Some(tab.clone());
            }
            tab.connect_toggled({
                let stack = stack.clone();
                move |tab| {
                    if tab.is_active() {
                        stack.set_visible_child_name(name);
                    }
                }
            });
            tabs.append(&tab);
        }
        let controls = autark_common::chrome::controls(false);
        let top = gtk::CenterBox::new();
        top.add_css_class("top");
        top.set_start_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        top.set_center_widget(Some(&tabs));
        top.set_end_widget(Some(&controls));
        let handle = gtk::WindowHandle::builder().child(&top).build();

        // Alarm page.
        let alarm_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        alarm_list.add_css_class("alarms");
        let hour = spin(23.0, 7.0);
        let minute = spin(59.0, 0.0);
        let colon = gtk::Label::new(Some(":"));
        colon.add_css_class("colon");
        let clock_set = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        clock_set.set_halign(gtk::Align::Center);
        clock_set.append(&hour);
        clock_set.append(&colon);
        clock_set.append(&minute);
        let label = gtk::Entry::builder().placeholder_text("Label (optional)").build();
        label.add_css_class("field");
        let days_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        days_row.set_halign(gtk::Align::Center);
        let day_buttons: Vec<gtk::ToggleButton> = DAYS
            .iter()
            .map(|day| {
                let toggle = gtk::ToggleButton::with_label(&day[..1]);
                toggle.set_tooltip_text(Some(day));
                toggle.add_css_class("day");
                days_row.append(&toggle);
                toggle
            })
            .collect();
        let problem = gtk::Label::builder().wrap(true).visible(false).build();
        problem.add_css_class("problem");
        let save = button("Set alarm", "primary");
        let cancel = button("Cancel", "plain");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&save);
        let form_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        form_box.add_css_class("form");
        for widget in [clock_set.upcast_ref::<gtk::Widget>(), label.upcast_ref(), days_row.upcast_ref(), problem.upcast_ref(), buttons.upcast_ref()] {
            form_box.append(widget);
        }
        let form = gtk::Revealer::builder().child(&form_box).transition_type(gtk::RevealerTransitionType::SlideDown).build();
        let add = button("New alarm", "primary");
        add.set_halign(gtk::Align::Center);
        let list_scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&alarm_list).build();
        let alarm_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        alarm_page.add_css_class("page");
        alarm_page.append(&form);
        alarm_page.append(&list_scroller);
        alarm_page.append(&add);

        // Timer page.
        let (set_h, set_m, set_s) = (spin(23.0, 0.0), spin(59.0, 5.0), spin(59.0, 0.0));
        let timer_digits = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        timer_digits.set_halign(gtk::Align::Center);
        for (n, (spin, unit)) in [(&set_h, "h"), (&set_m, "min"), (&set_s, "s")].into_iter().enumerate() {
            if n > 0 {
                let colon = gtk::Label::new(Some(":"));
                colon.add_css_class("colon");
                timer_digits.append(&colon);
            }
            let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
            column.append(spin);
            let caption = gtk::Label::new(Some(unit));
            caption.add_css_class("unit");
            column.append(&caption);
            timer_digits.append(&column);
        }
        let presets = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        presets.set_halign(gtk::Align::Center);
        let start_timer = button("Start", "primary");
        let timer_setup = gtk::Box::new(gtk::Orientation::Vertical, 22);
        timer_setup.set_valign(gtk::Align::Center);
        timer_setup.append(&timer_digits);
        timer_setup.append(&presets);
        timer_setup.append(&start_timer);
        let timer_display = gtk::Label::new(Some("00:00"));
        timer_display.add_css_class("big");
        let cancel_timer = button("Cancel", "plain");
        let timer_running = gtk::Box::new(gtk::Orientation::Vertical, 22);
        timer_running.set_valign(gtk::Align::Center);
        timer_running.set_visible(false);
        timer_running.append(&timer_display);
        timer_running.append(&cancel_timer);
        let timer_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        timer_page.add_css_class("page");
        timer_page.append(&timer_setup);
        timer_page.append(&timer_running);

        // Stopwatch page.
        let watch_display = gtk::Label::new(Some("00:00.00"));
        watch_display.add_css_class("big");
        let watch_side = button("Reset", "plain");
        watch_side.set_sensitive(false);
        let watch_main = button("Start", "primary");
        let watch_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        watch_buttons.set_halign(gtk::Align::Center);
        watch_buttons.append(&watch_side);
        watch_buttons.append(&watch_main);
        let laps = gtk::Box::new(gtk::Orientation::Vertical, 0);
        laps.add_css_class("laps");
        let laps_scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&laps).build();
        let watch_page = gtk::Box::new(gtk::Orientation::Vertical, 22);
        watch_page.add_css_class("page");
        watch_page.append(&watch_display);
        watch_page.append(&watch_buttons);
        watch_page.append(&laps_scroller);

        stack.add_named(&alarm_page, Some("alarm"));
        stack.add_named(&timer_page, Some("timer"));
        stack.add_named(&watch_page, Some("stopwatch"));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&handle);
        content.append(&stack);
        window.set_child(Some(&content));

        let clock = Rc::new(Self {
            window,
            exe,
            state: RefCell::new(State::load()),
            stack,
            alarm_list,
            form,
            hour,
            minute,
            label,
            day_buttons,
            problem,
            set_h,
            set_m,
            set_s,
            timer_setup,
            timer_running,
            timer_display,
            start_timer,
            watch: RefCell::default(),
            watch_display,
            watch_main,
            watch_side,
            laps,
            ticks: Cell::new(0),
        });

        let weak = Rc::downgrade(&clock);
        let on = |button: &gtk::Button, act: fn(&Rc<Clock>)| {
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(clock) = weak.upgrade() {
                    act(&clock);
                }
            });
        };
        on(&add, |c| c.open_form());
        on(&cancel, |c| c.form.set_reveal_child(false));
        on(&save, |c| c.save_alarm());
        on(&clock.start_timer, |c| c.begin_timer(None));
        on(&cancel_timer, |c| c.cancel_timer());
        on(&clock.watch_main, |c| c.watch_toggle());
        on(&clock.watch_side, |c| c.watch_side());
        for (name, seconds) in [("1 min", 60), ("5 min", 300), ("10 min", 600), ("30 min", 1800)] {
            let preset = button(name, "plain");
            let weak = weak.clone();
            preset.connect_clicked(move |_| {
                if let Some(clock) = weak.upgrade() {
                    clock.begin_timer(Some(seconds));
                }
            });
            presets.append(&preset);
        }
        for spin in [&clock.set_h, &clock.set_m, &clock.set_s] {
            spin.connect_value_changed({
                let weak = weak.clone();
                move |_| {
                    if let Some(clock) = weak.upgrade() {
                        clock.start_timer.set_sensitive(clock.timer_seconds() > 0);
                    }
                }
            });
        }
        // Escape shuts the new-alarm form, if it's open.
        clock.expire_once_alarms();
        clock.refresh_alarms();
        clock.sync_timer();
        glib::timeout_add_local(Duration::from_millis(50), {
            let weak = weak.clone();
            move || match weak.upgrade() {
                Some(clock) => {
                    clock.tick();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
        let keep = clock.clone();
        clock.window.connect_close_request(move |_| {
            let _ = &keep;
            glib::Propagation::Proceed
        });
        clock
    }

    fn save(&self) {
        self.state.borrow().save();
    }

    // Alarms.

    fn open_form(&self) {
        if let Ok(time) = glib::DateTime::now_local() {
            let next = time.add_minutes(1).unwrap_or(time);
            self.hour.set_value(f64::from(next.hour()));
            self.minute.set_value(f64::from(next.minute()));
        }
        self.label.set_text("");
        self.problem.set_visible(false);
        for day in &self.day_buttons {
            day.set_active(false);
        }
        self.form.set_reveal_child(true);
    }

    /// Today's date and the minute of the day, as the local clock has them.
    fn local_now() -> Option<((i32, u32, u32), u32)> {
        let time = glib::DateTime::now_local().ok()?;
        Some(((time.year(), time.month() as u32, time.day_of_month() as u32), (time.hour() * 60 + time.minute()) as u32))
    }

    fn date_for(hour: u8, minute: u8) -> Option<String> {
        let (today, minutes) = Self::local_now()?;
        let (year, month, day) = schedule::next_date(today, minutes, hour, minute);
        Some(format!("{year:04}-{month:02}-{day:02}"))
    }

    fn save_alarm(self: &Rc<Self>) {
        let days: Vec<String> = DAYS.iter().zip(&self.day_buttons).filter(|(_, toggle)| toggle.is_active()).map(|(day, _)| (*day).to_owned()).collect();
        let (hour, minute) = (self.hour.value_as_int() as u8, self.minute.value_as_int() as u8);
        let date = if days.is_empty() { Self::date_for(hour, minute) } else { None };
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64);
        let alarm = Alarm { id: millis, hour, minute, days, date, label: self.label.text().trim().to_owned(), enabled: true };
        if let Err(error) = schedule::apply(&alarm, &self.exe) {
            self.problem.set_text(&format!("Couldn’t set the alarm: {error}"));
            self.problem.set_visible(true);
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.alarms.push(alarm);
            state.alarms.sort_by_key(|alarm| (alarm.hour, alarm.minute));
        }
        self.save();
        self.form.set_reveal_child(false);
        self.refresh_alarms();
    }

    /// A one-time alarm whose time has passed has rung: it's turned off, ready to switch on again.
    fn expire_once_alarms(&self) {
        let Some((today, minutes)) = Self::local_now() else { return };
        let today = format!("{:04}-{:02}-{:02}", today.0, today.1, today.2);
        let mut changed = false;
        for alarm in self.state.borrow_mut().alarms.iter_mut().filter(|alarm| alarm.enabled && alarm.days.is_empty()) {
            let past = alarm.date.as_deref().is_some_and(|date| date < today.as_str() || (date == today.as_str() && u32::from(alarm.hour) * 60 + u32::from(alarm.minute) <= minutes));
            if past {
                alarm.enabled = false;
                let _ = schedule::apply(alarm, &self.exe);
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    fn refresh_alarms(self: &Rc<Self>) {
        while let Some(child) = self.alarm_list.first_child() {
            self.alarm_list.remove(&child);
        }
        let alarms = self.state.borrow().alarms.clone();
        if alarms.is_empty() {
            let empty = gtk::Label::new(Some("No alarms yet."));
            empty.add_css_class("empty");
            self.alarm_list.append(&empty);
        }
        for alarm in alarms {
            let time = gtk::Label::builder().label(format!("{:02}:{:02}", alarm.hour, alarm.minute)).xalign(0.0).build();
            time.add_css_class("alarm-time");
            let label = if alarm.label.is_empty() { "Alarm".to_owned() } else { alarm.label.clone() };
            let sub = gtk::Label::builder().label(format!("{label} · {}", days_text(&alarm.days))).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            sub.add_css_class("alarm-sub");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            text.set_hexpand(true);
            text.append(&time);
            text.append(&sub);
            let switch = gtk::Switch::builder().active(alarm.enabled).valign(gtk::Align::Center).build();
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("trash");
            delete.set_tooltip_text(Some("Delete"));
            delete.set_valign(gtk::Align::Center);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("alarm");
            if !alarm.enabled {
                row.add_css_class("off");
            }
            row.append(&text);
            row.append(&switch);
            row.append(&delete);
            self.alarm_list.append(&row);

            let id = alarm.id;
            let weak = Rc::downgrade(self);
            switch.connect_state_set({
                let weak = weak.clone();
                move |_, on| {
                    if let Some(clock) = weak.upgrade() {
                        clock.set_enabled(id, on);
                    }
                    glib::Propagation::Proceed
                }
            });
            delete.connect_clicked(move |_| {
                if let Some(clock) = weak.upgrade() {
                    let _ = schedule::remove(id);
                    clock.state.borrow_mut().alarms.retain(|alarm| alarm.id != id);
                    clock.save();
                    clock.refresh_alarms();
                }
            });
        }
    }

    fn set_enabled(self: &Rc<Self>, id: u64, on: bool) {
        let alarm = {
            let mut state = self.state.borrow_mut();
            let Some(alarm) = state.alarms.iter_mut().find(|alarm| alarm.id == id) else { return };
            alarm.enabled = on;
            // Switching a one-time alarm back on means its next time.
            if on && alarm.days.is_empty() {
                alarm.date = Self::date_for(alarm.hour, alarm.minute);
            }
            alarm.clone()
        };
        if let Err(error) = schedule::apply(&alarm, &self.exe) {
            eprintln!("clock: {error}");
        }
        self.save();
        // Redraw after the switch has finished moving.
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(clock) = weak.upgrade() {
                clock.refresh_alarms();
            }
        });
    }

    // Timer.

    fn timer_seconds(&self) -> u64 {
        (self.set_h.value_as_int() * 3600 + self.set_m.value_as_int() * 60 + self.set_s.value_as_int()) as u64
    }

    /// Starts a timer of `seconds`, or of what the three fields say.
    fn begin_timer(&self, seconds: Option<u64>) {
        let seconds = seconds.unwrap_or_else(|| self.timer_seconds());
        if seconds == 0 {
            return;
        }
        match schedule::start_timer(seconds, &self.exe) {
            Ok(timer) => {
                self.state.borrow_mut().timer = Some(timer);
                self.save();
                self.sync_timer();
            }
            Err(error) => {
                self.timer_display.set_text(&format!("Couldn’t start: {error}"));
                self.timer_setup.set_visible(false);
                self.timer_running.set_visible(true);
            }
        }
    }

    fn cancel_timer(&self) {
        if let Some(timer) = self.state.borrow_mut().timer.take() {
            schedule::stop_timer(&timer.unit);
        }
        self.save();
        self.sync_timer();
    }

    /// Shows the countdown while one runs, and the fields otherwise.
    fn sync_timer(&self) {
        let remaining = self.state.borrow().timer.as_ref().map(|timer: &Timer| timer.end.saturating_sub(now()));
        match remaining {
            Some(left) if left > 0 => {
                self.timer_display.set_text(&countdown(left));
                self.timer_setup.set_visible(false);
                self.timer_running.set_visible(true);
            }
            Some(_) => {
                // It has rung; say so for a moment, then clear it.
                self.timer_display.set_text("Time’s up");
                self.timer_setup.set_visible(false);
                self.timer_running.set_visible(true);
            }
            None => {
                self.timer_setup.set_visible(true);
                self.timer_running.set_visible(false);
            }
        }
    }

    // Stopwatch.

    fn watch_toggle(&self) {
        {
            let mut watch = self.watch.borrow_mut();
            match watch.started.take() {
                Some(started) => watch.before += started.elapsed(),
                None => watch.started = Some(Instant::now()),
            }
        }
        self.sync_watch();
    }

    /// Lap while running, reset while stopped.
    fn watch_side(&self) {
        {
            let mut watch = self.watch.borrow_mut();
            if watch.started.is_some() {
                let at = watch.elapsed();
                watch.laps.push(at);
            } else {
                *watch = Watch::default();
            }
        }
        self.sync_watch();
    }

    fn sync_watch(&self) {
        let watch = self.watch.borrow();
        let running = watch.started.is_some();
        self.watch_main.set_label(if running { "Stop" } else { "Start" });
        self.watch_main.remove_css_class("primary");
        self.watch_main.remove_css_class("danger");
        self.watch_main.add_css_class(if running { "danger" } else { "primary" });
        self.watch_side.set_label(if running { "Lap" } else { "Reset" });
        self.watch_side.set_sensitive(running || watch.elapsed() > Duration::ZERO);
        self.watch_display.set_text(&stopwatch(watch.elapsed()));
        while let Some(child) = self.laps.first_child() {
            self.laps.remove(&child);
        }
        let mut earlier = Duration::ZERO;
        for (n, lap) in watch.laps.iter().enumerate().rev().map(|(n, lap)| (n, *lap)) {
            let before = if n == 0 { Duration::ZERO } else { watch.laps[n - 1] };
            let split = lap - before;
            let _ = earlier;
            earlier = lap;
            let name = gtk::Label::builder().label(format!("Lap {}", n + 1)).xalign(0.0).hexpand(true).build();
            let split_label = gtk::Label::new(Some(&stopwatch(split)));
            split_label.add_css_class("mono");
            let total = gtk::Label::new(Some(&stopwatch(lap)));
            total.add_css_class("mono");
            total.add_css_class("dim");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.add_css_class("lap");
            row.append(&name);
            row.append(&split_label);
            row.append(&total);
            self.laps.append(&row);
        }
    }

    /// About 20 times a second: the stopwatch's numbers, and now and then the timer's.
    fn tick(&self) {
        if self.watch.borrow().started.is_some() {
            self.watch_display.set_text(&stopwatch(self.watch.borrow().elapsed()));
        }
        let ticks = self.ticks.get() + 1;
        self.ticks.set(if ticks >= 4 { 0 } else { ticks });
        if ticks >= 4 && self.state.borrow().timer.is_some() {
            self.sync_timer();
            // A little after it rings, the timer is cleared.
            let over = self.state.borrow().timer.as_ref().is_some_and(|timer| now() >= timer.end + 4);
            if over {
                self.state.borrow_mut().timer = None;
                self.save();
                self.sync_timer();
            }
        }
        let _ = &self.stack;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_a_clock_shows_them() {
        assert_eq!(countdown(65), "01:05");
        assert_eq!(countdown(3665), "1:01:05");
        assert_eq!(countdown(0), "00:00");
        assert_eq!(stopwatch(Duration::from_millis(65_250)), "01:05.25");
        assert_eq!(stopwatch(Duration::from_millis(3_665_999)), "1:01:05.99");
    }

    #[test]
    fn days_read_as_people_say_them() {
        let days = |list: &[&str]| list.iter().map(|day| (*day).to_owned()).collect::<Vec<_>>();
        assert_eq!(days_text(&[]), "Once");
        assert_eq!(days_text(&days(&["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"])), "Every day");
        assert_eq!(days_text(&days(&["Fri", "Mon", "Wed", "Thu", "Tue"])), "Weekdays");
        assert_eq!(days_text(&days(&["Sun", "Sat"])), "Weekends");
        assert_eq!(days_text(&days(&["Wed", "Mon"])), "Mon, Wed");
    }
}
