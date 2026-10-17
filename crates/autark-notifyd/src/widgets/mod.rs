//! Widgets, built in: the centre's Widgets view and the optional desktop layer. Each source is
//! read only while something shows it, except Kith's suggestions, followed as `autark-ai` makes them.

mod battery;
mod clock;
mod kith;
mod note;
mod resources;
mod settings;
mod weather;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::config::{Location, Notifyd};
use crate::shared::Shared;
use resources::Feed;
use settings::Settings;

/// How often batteries and tasks are reread while shown.
const REFRESH: Duration = Duration::from_secs(30);
const WEATHER_EVERY: Duration = Duration::from_secs(30 * 60);
/// How often suggestions are read again with nothing changed, as each stands for an hour.
const SUGGESTIONS_AGAIN: Duration = Duration::from_secs(5 * 60);

pub struct Sources {
    now: Shared<glib::DateTime>,
    batteries: Shared<Vec<battery::Battery>>,
    feed: Shared<Feed>,
    tasks: Shared<Vec<kith::Task>>,
    suggestion: Shared<Option<kith::Suggestion>>,
    /// What Kith is doing, for its face.
    kith: Shared<autark_common::visor::State>,
    note: Rc<note::Source>,
    weather: Shared<Option<weather::Weather>>,
    settings: Shared<Settings>,
    location: Option<Location>,
    /// Dropped to stop what runs while the widgets are shown.
    showing: RefCell<Option<async_channel::Sender<()>>>,
    weather_fetched: Cell<Option<Instant>>,
    _watch: RefCell<Option<autark_common::watch::Watch>>,
    me: Weak<Sources>,
}

impl Sources {
    pub fn new(config: &Notifyd) -> Rc<Self> {
        let sources = Rc::new_cyclic(|me| Self {
            now: clock::source(),
            batteries: Shared::new(Vec::new()),
            feed: Shared::new(Feed::Waiting),
            tasks: Shared::new(Vec::new()),
            suggestion: Shared::new(None),
            kith: Shared::new(autark_common::visor::State::Idle),
            note: note::Source::new(config.note.clone()),
            weather: Shared::new(None),
            settings: Shared::new(Settings::load()),
            location: config.weather.clone(),
            showing: RefCell::default(),
            weather_fetched: Cell::new(None),
            _watch: RefCell::default(),
            me: me.clone(),
        });
        let me = Rc::downgrade(&sources);
        *sources._watch.borrow_mut() = Some(autark_common::watch::files(&[settings::file()], move || {
            if let Some(sources) = me.upgrade() {
                let settings = Settings::load();
                if settings != sources.settings.get() {
                    sources.settings.set(settings);
                }
            }
        }));
        if config.suggestions {
            glib::spawn_future_local(follow_suggestions(Rc::downgrade(&sources)));
        }
        sources
    }

    /// Starts or stops reading for the centre's Widgets view.
    pub fn set_shown(&self, shown: bool) {
        if !shown {
            // Closing the channel ends every loop waiting on it.
            self.showing.take();
            return;
        }
        if self.showing.borrow().is_some() {
            return;
        }
        let (stop, stopped) = async_channel::bounded(1);
        *self.showing.borrow_mut() = Some(stop);
        self.now.set(clock::local_now());
        self.note.reload();
        self.update_weather();
        glib::spawn_future_local(resources::follow(self.feed.clone(), stopped.clone()));
        let me = self.me.clone();
        glib::spawn_future_local(async move {
            let system = zbus::Connection::system().await;
            let session = zbus::Connection::session().await;
            loop {
                let Some(sources) = me.upgrade() else { return };
                if let Ok(system) = &system {
                    match battery::read(system).await {
                        Ok(batteries) => sources.batteries.set(batteries),
                        Err(err) => {
                            eprintln!("notifyd: reading batteries from UPower: {err}");
                            sources.batteries.set(Vec::new());
                        }
                    }
                }
                if let Ok(session) = &session {
                    match kith::tasks(session).await {
                        Ok(tasks) => sources.tasks.set(tasks),
                        Err(err) => eprintln!("notifyd: listing Kith's task timers: {err}"),
                    }
                }
                drop(sources);
                let wait = std::pin::pin!(glib::timeout_future(REFRESH));
                if let futures_util::future::Either::Right(_) = futures_util::future::select(wait, std::pin::pin!(stopped.recv())).await {
                    return;
                }
            }
        });
    }

    fn weather_on(&self) -> bool {
        self.settings.get().weather && self.location.is_some()
    }

    fn update_weather(&self) {
        let Some(location) = self.location.clone().filter(|_| self.weather_on()) else {
            self.weather.set(None);
            return;
        };
        if self.weather_fetched.get().is_some_and(|at| at.elapsed() < WEATHER_EVERY) {
            return;
        }
        self.weather_fetched.set(Some(Instant::now()));
        let me = self.me.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || weather::fetch(&location)).await;
            let Some(sources) = me.upgrade() else { return };
            match result {
                Ok(Ok(weather)) if sources.weather_on() => sources.weather.set(Some(weather)),
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    eprintln!("notifyd: weather from Open-Meteo: {err}");
                    sources.weather_fetched.set(None);
                }
                Err(_) => sources.weather_fetched.set(None),
            }
        });
    }

    fn change_settings(&self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.settings.get();
        change(&mut settings);
        settings.save();
        self.settings.set(settings);
    }

    fn answer(&self, answer: kith::Answer) {
        let Some(suggestion) = self.suggestion.get() else { return };
        self.suggestion.set(None);
        glib::spawn_future_local(async move {
            let answered = async { kith::AiProxy::new(&zbus::Connection::session().await?).await?.answer_suggestion(suggestion.entry, answer.word()).await };
            match answered.await {
                Ok(result) if !result.is_empty() => eprintln!("notifyd: {}: {result}", suggestion.offer),
                Ok(_) => {}
                Err(err) => eprintln!("notifyd: answering Kith's suggestion: {err}"),
            }
        });
    }
}

/// Reads Kith's open suggestions whenever its conversation changes, and every few minutes, since each
/// stands only an hour.
async fn follow_suggestions(sources: Weak<Sources>) {
    let followed = async {
        let connection = zbus::Connection::session().await?;
        let ai = kith::AiProxy::new(&connection).await?;
        let mut changes = ai.receive_history_changed().await?;
        let mut states = ai.receive_state_changed().await?;
        if let (Some(sources), Ok(state)) = (sources.upgrade(), ai.state().await) {
            sources.kith.set(autark_common::visor::State::from_word(&state));
        }
        loop {
            let Some(live) = sources.upgrade() else { return Ok::<(), zbus::Error>(()) };
            let found = ai.open_suggestions().await.ok().and_then(|json| kith::newest(&json));
            if live.suggestion.get() != found {
                live.suggestion.set(found);
            }
            drop(live);
            // Kith's state changes often while it answers, so it's handled here without reading suggestions again.
            loop {
                let again = std::pin::pin!(glib::timeout_future(SUGGESTIONS_AGAIN));
                let state = std::pin::pin!(states.next());
                let changed = std::pin::pin!(changes.next());
                match futures_util::future::select(futures_util::future::select(again, changed), state).await {
                    futures_util::future::Either::Right((Some(signal), _)) => {
                        let Some(live) = sources.upgrade() else { return Ok(()) };
                        live.kith.set(autark_common::visor::State::from_word(&signal.args()?.state));
                    }
                    _ => break,
                }
            }
        }
    };
    if let Err(err) = followed.await {
        eprintln!("notifyd: following Kith's suggestions: {err}");
    }
}

/// A small grey heading, as on every card.
pub fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("caption");
    label
}

/// Shows `widget` while `shown` says so, rechecked whenever `settings` or `extra` changes.
fn show_while<T: 'static>(widget: &impl IsA<gtk::Widget>, settings: &Shared<Settings>, extra: &Shared<T>, shown: impl Fn(&Settings, &T) -> bool + 'static) {
    let shown = Rc::new(shown);
    let (settings_now, extra_now) = (settings.clone(), extra.clone());
    let check = Rc::new(move |widget: &gtk::Widget| {
        let visible = settings_now.with(|settings| extra_now.with(|extra| shown(settings, extra)));
        widget.set_visible(visible);
    });
    let widget = widget.clone().upcast::<gtk::Widget>();
    let first = check.clone();
    settings.watch(&widget, move |widget, _| first(widget));
    extra.watch(&widget, move |widget, _| check(widget));
}

/// The centre's Widgets view.
/// The Widgets view, and its Edit widgets button.
pub fn page(sources: &Rc<Sources>) -> (gtk::Box, gtk::MenuButton) {
    let settings = &sources.settings;
    let clock = clock::view(&sources.now, false);
    show_while(&clock, settings, settings, |settings, _| settings.shows("clock"));

    let batteries = battery::view(&sources.batteries);
    show_while(&batteries, settings, &sources.batteries, |settings, batteries| settings.shows("battery") && !batteries.is_empty());
    let resources = resources::view(&sources.feed);
    show_while(&resources, settings, settings, |settings, _| settings.shows("resources"));
    let pair = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    pair.set_homogeneous(true);
    pair.append(&batteries);
    pair.append(&resources);

    let weather = weather::view(&sources.weather);
    let me = Rc::downgrade(sources);
    let kith = kith::view(&sources.tasks, &sources.now, &sources.suggestion, &sources.kith, move |answer| {
        if let Some(sources) = me.upgrade() {
            sources.answer(answer);
        }
    });
    show_while(&kith, settings, settings, |settings, _| settings.shows("kith"));
    let note = note::view(&sources.note.note);
    show_while(&note, settings, settings, |settings, _| settings.shows("note"));

    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.add_css_class("widgets");
    page.append(&clock);
    page.append(&pair);
    page.append(&weather);
    page.append(&kith);
    page.append(&note);
    let (footer, edit) = footer(sources);
    page.append(&footer);
    (page, edit)
}

fn footer(sources: &Rc<Sources>) -> (gtk::Box, gtk::MenuButton) {
    let note = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
    note.add_css_class("detail");
    let edit = gtk::MenuButton::builder().label("Edit widgets").popover(&edit_popover(sources)).build();
    edit.add_css_class("edit");
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("footer");
    footer.append(&note);
    footer.append(&edit);
    let has_location = sources.location.is_some();
    sources.settings.watch(&note, move |note, settings| {
        note.set_label(if settings.weather && has_location { "Weather from Open-Meteo" } else { "Weather is off: it sends your location" });
    });
    (footer, edit)
}

type OnSwitch = Box<dyn Fn(&Sources, bool)>;

fn edit_popover(sources: &Rc<Sources>) -> gtk::Popover {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("edit-list");
    let settings = sources.settings.get();
    let row = |name: &str, active: bool, on_change: OnSwitch| {
        let label = gtk::Label::builder().label(name).xalign(0.0).hexpand(true).build();
        let switch = gtk::Switch::builder().active(active).valign(gtk::Align::Center).build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.add_css_class("edit-row");
        line.append(&label);
        line.append(&switch);
        let me = Rc::downgrade(sources);
        switch.connect_active_notify(move |switch| {
            if let Some(sources) = me.upgrade() {
                on_change(&sources, switch.is_active());
            }
        });
        (line, switch)
    };
    for (kind, name) in settings::KINDS {
        let (line, _) = row(
            name,
            settings.shows(kind),
            Box::new(move |sources, on| {
                sources.change_settings(|settings| {
                    settings.hidden.retain(|hidden| hidden != kind);
                    if !on {
                        settings.hidden.push(kind.to_owned());
                    }
                })
            }),
        );
        list.append(&line);
    }
    let (weather, switch) = row(
        "Weather",
        settings.weather,
        Box::new(|sources, on| {
            sources.change_settings(|settings| settings.weather = on);
            sources.weather_fetched.set(None);
            sources.update_weather();
        }),
    );
    if sources.location.is_none() {
        switch.set_sensitive(false);
        weather.set_tooltip_text(Some("Set [weather] with your latitude and longitude in notifyd.toml first"));
    }
    list.append(&weather);
    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let (desktop, _) = row("Show on desktop", settings.desktop, Box::new(|sources, on| sources.change_settings(|settings| settings.desktop = on)));
    list.append(&desktop);

    let popover = gtk::Popover::builder().child(&list).build();
    popover.add_css_class("edit-widgets");
    popover
}

/// The desktop layer: the clock and the pinned note at the top left, under every window.
pub fn desktop(app: &gtk::Application, sources: &Rc<Sources>) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::new(app);
    window.init_layer_shell();
    window.set_layer(Layer::Bottom);
    window.set_namespace(Some("autark-desktop-widgets"));
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_margin(Edge::Top, 28);
    window.set_margin(Edge::Left, 32);
    window.set_keyboard_mode(KeyboardMode::None);
    window.add_css_class("notifications");
    window.add_css_class("desktop-widgets");

    let settings = &sources.settings;
    let clock = clock::view(&sources.now, true);
    show_while(&clock, settings, settings, |settings, _| settings.shows("clock"));
    let note = note::desktop_view(&sources.note.note);
    show_while(&note, settings, &sources.note.note, |settings, note| settings.shows("note") && note.is_some());
    let column = gtk::Box::new(gtk::Orientation::Vertical, 14);
    column.set_size_request(344, -1);
    column.append(&clock);
    column.append(&note);
    window.set_child(Some(&column));

    show_while(&window, settings, &sources.note.note, |settings, note| settings.desktop && (settings.shows("clock") || (settings.shows("note") && note.is_some())));
    window
}
