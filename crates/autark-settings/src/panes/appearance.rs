//! Appearance: edits `theme.toml`, which every Autark tool and the wallpaper watch, and passes the theme on to
//! other GTK apps through their `settings.ini`. The pane follows the file too, so an edit by hand shows here.

use std::cell::RefCell;
use std::rc::Rc;

use autark_common::theme::{self, ACCENTS, Look, Mode, Theme};
use autark_common::wallpaper::{self, SCENES};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{config, ui};

/// The theme cards' colours: ground, bar and window, as in the design.
const LIGHT_CARD: [&str; 3] = ["#F6F4EF", "#E2DDD3", "#FFFFFF"];
const DARK_CARD: [&str; 3] = ["#1A1916", "#26241F", "#2E2B26"];
const THEMES: [(&str, &str); 3] = [("light", "Light"), ("dark", "Dark"), ("auto", "Auto")];
const SIZES: [(&str, &str); 3] = [("100", "Default"), ("115", "Larger"), ("130", "Largest")];
const FONTS: [(&str, &str); 2] = [("Geist", "Geist"), ("Inter", "Inter")];
const PICTURE: &str = "picture";
/// A little smaller than the design's 140 by 80, so the picture card fits beside the four scenes.
const WALL: (i32, i32) = (128, 73);

struct Appearance {
    /// Each card or dot with the value it stands for, to mark the chosen one.
    themes: Vec<(&'static str, gtk::Button)>,
    accents: Vec<(&'static str, gtk::Button)>,
    walls: Vec<(&'static str, gtk::Button)>,
    accent_label: gtk::Label,
    /// The picture card's face, which shows the chosen picture.
    picture: gtk::Picture,
    status: gtk::Label,
    /// Set while the controls are moved to match the file, so that isn't written back.
    syncing: std::cell::Cell<bool>,
    switches: RefCell<Vec<(&'static str, gtk::Switch)>>,
    choices: RefCell<Vec<(&'static str, gtk::DropDown)>>,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Appearance", None));
    let look = theme::look();
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);

    let accent_label = ui::note("");
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    let pane = Rc::new_cyclic(|pane: &std::rc::Weak<Appearance>| {
        let themes = THEMES
            .iter()
            .map(|&(id, label)| {
                let face = gtk::DrawingArea::builder().content_width(140).content_height(88).build();
                face.set_draw_func(move |_, cr, width, height| draw_theme(cr, id, f64::from(width), f64::from(height)));
                theme::redraw_on_change(&face);
                let pane = pane.clone();
                (id, card(&face, label, move || set(&pane, "theme", id)))
            })
            .collect();
        let accents = ACCENTS
            .iter()
            .map(|&(id, ..)| {
                let face = gtk::DrawingArea::builder().content_width(28).content_height(28).build();
                face.set_draw_func(move |_, cr, width, height| {
                    let look = theme::look();
                    let hex = ACCENTS.iter().find(|accent| accent.0 == id).map_or("#3F7A5F", |accent| if look.dark { accent.2 } else { accent.1 });
                    theme::set_source(cr, hex, 1.0);
                    cr.arc(f64::from(width) / 2.0, f64::from(height) / 2.0, f64::from(width.min(height)) / 2.0, 0.0, std::f64::consts::TAU);
                    cr.fill().ok();
                });
                theme::redraw_on_change(&face);
                let button = gtk::Button::builder().child(&face).css_classes(["dot"]).tooltip_text(title(id)).build();
                button.update_property(&[gtk::accessible::Property::Label(&title(id))]);
                let pane = pane.clone();
                button.connect_clicked(move |_| set(&pane, "accent", id));
                (id, button)
            })
            .collect();
        let mut walls: Vec<(&str, gtk::Button)> = SCENES
            .iter()
            .map(|scene| {
                let face = gtk::DrawingArea::builder().content_width(WALL.0).content_height(WALL.1).build();
                face.set_draw_func(move |_, cr, width, height| wallpaper::draw(cr, scene, f64::from(width), f64::from(height)));
                let pane = pane.clone();
                (scene.id, card(&face, scene.label, move || set(&pane, "wallpaper", scene.id)))
            })
            .collect();
        let face = gtk::Overlay::builder().width_request(WALL.0).height_request(WALL.1).child(&picture).build();
        let plus = gtk::Image::from_icon_name("list-add-symbolic");
        plus.add_css_class("choose");
        face.add_overlay(&plus);
        let pane_for_picture = pane.clone();
        walls.push((PICTURE, card(&face, "Picture…", move || choose_picture(&pane_for_picture))));
        Appearance { themes, accents, walls, accent_label, picture, status: status.clone(), syncing: Default::default(), switches: RefCell::default(), choices: RefCell::default() }
    });

    let row = |cards: &[(&str, gtk::Button)], spacing: i32| {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
        for (_, button) in cards {
            line.append(button);
        }
        line
    };
    column.append(&labelled("Theme", &row(&pane.themes, 16)));
    let dots = row(&pane.accents, 14);
    pane.accent_label.set_margin_start(6);
    dots.append(&pane.accent_label);
    column.append(&labelled("Accent colour", &dots));
    column.append(&labelled("Wallpaper", &row(&pane.walls, 12)));

    let group = ui::group();
    let document = config::read("theme");
    let size = config::get_int(&document, "text_size").unwrap_or(100).to_string();
    let font = config::get_str(&document, "font").unwrap_or_else(|| "Geist".into());
    for (key, label, options, current) in [("text_size", "Text size", &SIZES[..], size), ("font", "Font", &FONTS[..], font)] {
        let pane_for_choice = Rc::downgrade(&pane);
        let dropdown = ui::choice(options, &current, move |value| {
            let value = value.to_owned();
            if key == "text_size" {
                set_value(&pane_for_choice, key, value.parse::<i64>().unwrap_or(100).into());
            } else {
                set_value(&pane_for_choice, key, value.as_str().into());
            }
        });
        dropdown.update_property(&[gtk::accessible::Property::Label(label)]);
        group.append(&ui::row(label, None, &dropdown));
        pane.choices.borrow_mut().push((key, dropdown));
    }
    for (key, label, note) in [("reduce_motion", "Reduce motion", "No magnifying dock, sliding panels or fades"), ("contrast", "Increase contrast", "A high-contrast Linen: darker text and borders")] {
        let pane_for_switch = Rc::downgrade(&pane);
        let switch = ui::switch(config::get_bool(&document, key).unwrap_or(false), move |on| set_value(&pane_for_switch, key, on.into()));
        switch.update_property(&[gtk::accessible::Property::Label(label)]);
        group.append(&ui::row(label, Some(note), &switch));
        pane.switches.borrow_mut().push((key, switch));
    }
    column.append(&ui::section("Accessibility", None, &group));
    column.append(&status);

    pane.show(&look);
    // Kept by the listener, as a page once built stays for as long as Settings is open.
    theme::on_change(move |look| pane.show(look));
    page.upcast()
}

impl Appearance {
    /// Marks what `look` chose, and moves the controls to match.
    fn show(&self, look: &Look) {
        let chosen = &look.theme;
        let mode = match chosen.theme {
            Mode::Light => "light",
            Mode::Dark => "dark",
            Mode::Auto => "auto",
        };
        mark(&self.themes, mode);
        mark(&self.accents, &chosen.accent);
        self.accent_label.set_label(&title(&chosen.accent));
        let picked = if wallpaper::is_picture(&chosen.wallpaper) { PICTURE } else { chosen.wallpaper.as_str() };
        mark(&self.walls, picked);
        if wallpaper::is_picture(&chosen.wallpaper) {
            self.picture.set_filename(Some(wallpaper::picture_path(&chosen.wallpaper)));
        } else {
            self.picture.set_paintable(gdk::Paintable::NONE);
        }
        self.syncing.set(true);
        for (key, switch) in self.switches.borrow().iter() {
            switch.set_active(if *key == "contrast" { chosen.contrast } else { chosen.reduce_motion });
        }
        for (key, dropdown) in self.choices.borrow().iter() {
            let index = match *key {
                "text_size" => SIZES.iter().position(|(value, _)| *value == chosen.text_size.to_string()),
                _ => FONTS.iter().position(|(value, _)| *value == chosen.font),
            };
            if let Some(index) = index {
                dropdown.set_selected(index as u32);
            }
        }
        self.syncing.set(false);
    }

    fn report(&self, result: Result<(), String>) {
        self.status.set_visible(result.is_err());
        if let Err(error) = result {
            self.status.set_label(&error);
        }
    }
}

/// A card: its face in a rounded frame with the label below, ringed in the accent when chosen.
fn card(face: &impl IsA<gtk::Widget>, label: &str, picked: impl Fn() + 'static) -> gtk::Button {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("card-face");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_halign(gtk::Align::Center);
    frame.append(face);
    let text = gtk::Label::new(Some(label));
    text.add_css_class("card-label");
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    inner.append(&frame);
    inner.append(&text);
    let button = gtk::Button::builder().child(&inner).css_classes(["card"]).build();
    button.connect_clicked(move |_| picked());
    button
}

fn mark(buttons: &[(&str, gtk::Button)], chosen: &str) {
    for (id, button) in buttons {
        let on = *id == chosen;
        if on {
            button.add_css_class("picked");
        } else {
            button.remove_css_class("picked");
        }
        button.update_state(&[gtk::accessible::State::Pressed(if on { gtk::AccessibleTristate::True } else { gtk::AccessibleTristate::False })]);
    }
}

fn labelled(heading: &str, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let label = gtk::Label::new(Some(heading));
    label.add_css_class("section-heading");
    label.set_xalign(0.0);
    section.append(&label);
    section.append(body);
    section
}

/// "sage" as "Sage".
fn title(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A theme card: a little desktop with a bar, a window and an accent stroke; Auto is half light, half dark.
fn draw_theme(cr: &gtk::cairo::Context, id: &str, width: f64, height: f64) {
    let accent = theme::look().palette.accent.clone();
    let halves: &[(f64, [&str; 3])] = match id {
        "dark" => &[(0.0, DARK_CARD)],
        "auto" => &[(0.0, LIGHT_CARD), (0.5, DARK_CARD)],
        _ => &[(0.0, LIGHT_CARD)],
    };
    for &(from, [ground, bar, window]) in halves {
        cr.save().ok();
        cr.rectangle(width * from, 0.0, width, height);
        cr.clip();
        theme::set_source(cr, ground, 1.0);
        cr.paint().ok();
        theme::set_source(cr, bar, 1.0);
        cr.rectangle(0.0, 0.0, width, 9.0);
        cr.fill().ok();
        theme::set_source(cr, window, 1.0);
        rounded(cr, 18.0, 22.0, 92.0, 52.0, 6.0);
        cr.fill().ok();
        cr.restore().ok();
    }
    theme::set_source(cr, &accent, 1.0);
    rounded(cr, 28.0, 34.0, 40.0, 6.0, 3.0);
    cr.fill().ok();
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn set(pane: &std::rc::Weak<Appearance>, key: &'static str, value: &str) {
    set_value(pane, key, value.into());
}

/// Writes `key` to `theme.toml` unless it's already so, then passes the theme on to GTK's settings. The
/// pane catches up when the file's watch says it changed.
fn set_value(pane: &std::rc::Weak<Appearance>, key: &'static str, value: toml_edit::Value) {
    let Some(pane) = pane.upgrade() else { return };
    if pane.syncing.get() {
        return;
    }
    let document = config::read("theme");
    let same = match (&value, key) {
        (toml_edit::Value::String(text), _) => config::get_str(&document, key).as_deref() == Some(text.value().as_str()),
        (toml_edit::Value::Boolean(on), _) => config::get_bool(&document, key) == Some(*on.value()),
        (toml_edit::Value::Integer(number), _) => config::get_int(&document, key) == Some(*number.value()),
        _ => false,
    };
    if same {
        return;
    }
    let written = config::edit("theme", |document| config::set_in(document, key, value));
    let written = written.and_then(|()| theme::write_gtk_settings(&Theme::load()).map_err(|error| format!("GTK’s settings.ini: {error}")));
    pane.report(written);
}

/// Opens a file chooser for a picture, and makes it the wallpaper.
fn choose_picture(pane: &std::rc::Weak<Appearance>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Pictures"));
    filter.add_pixbuf_formats();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title("Choose a picture").modal(true).filters(&filters).default_filter(&filter).build();
    let pictures = glib::user_special_dir(glib::UserDirectory::Pictures);
    if let Some(folder) = pictures {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    let window = pane.upgrade().and_then(|pane| pane.picture.root()).and_downcast::<gtk::Window>();
    let pane = pane.clone();
    dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| match chosen {
        Ok(file) => {
            if let Some(path) = file.path() {
                set(&pane, "wallpaper", &path.to_string_lossy());
            }
        }
        Err(error) if error.matches(gtk::DialogError::Dismissed) || error.matches(gtk::DialogError::Cancelled) => {}
        Err(error) => {
            if let Some(pane) = pane.upgrade() {
                pane.report(Err(format!("Couldn’t open the file chooser: {error}")));
            }
        }
    });
}
