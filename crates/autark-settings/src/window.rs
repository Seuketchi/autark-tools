//! The window: the sidebar (search, the panes) and the open pane. Panes are built the first
//! time they're opened and kept, each keeping itself current.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::panes::{self, INDEX, PANES};
use crate::{icon, ui};

pub struct Shell {
    pub window: gtk::ApplicationWindow,
    nav: Vec<(&'static str, gtk::Button)>,
    browse: gtk::Box,
    results: gtk::Box,
    pages: gtk::Stack,
    search: gtk::SearchEntry,
    first: RefCell<Option<&'static str>>,
}

impl Shell {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).default_width(1104).default_height(700).title("Settings").build();
        window.add_css_class("settings");
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let search = gtk::SearchEntry::builder().placeholder_text("Search").build();
        search.add_css_class("search");

        let browse = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let mut nav = Vec::new();
        for pane in &PANES {
            if pane.gap {
                let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
                gap.set_size_request(-1, 10);
                browse.append(&gap);
            }
            let button = nav_button(pane);
            browse.append(&button);
            nav.push((pane.id, button));
        }
        let results = gtk::Box::new(gtk::Orientation::Vertical, 2);
        results.set_visible(false);

        let lists = gtk::Box::new(gtk::Orientation::Vertical, 0);
        lists.append(&browse);
        lists.append(&results);
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.add_css_class("sidebar");
        side.set_size_request(252, -1);
        side.set_hexpand(false);
        side.append(&search);
        side.append(&gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&lists).build());

        let pages = gtk::Stack::builder().hexpand(true).vexpand(true).transition_type(gtk::StackTransitionType::None).build();
        let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        main.append(&side);
        // The window buttons float at the pane's top right, above the pane's own title.
        let controls = autark_common::chrome::controls(true);
        controls.set_valign(gtk::Align::Start);
        controls.set_margin_top(10);
        controls.set_margin_end(12);
        let over = gtk::Overlay::builder().child(&pages).hexpand(true).build();
        over.add_overlay(&controls);
        main.append(&gtk::WindowHandle::builder().child(&over).hexpand(true).build());
        window.set_child(Some(&main));

        let shell = Rc::new(Self { window, nav, browse, results, pages, search, first: RefCell::new(None) });
        // `win.show-pane` with a pane's id opens it, for a pane that points at another (Network at Wi-Fi).
        let open = gio::SimpleAction::new("show-pane", Some(glib::VariantTy::STRING));
        open.connect_activate({
            let shell = Rc::downgrade(&shell);
            move |_, id| {
                if let (Some(shell), Some(id)) = (shell.upgrade(), id.and_then(|id| id.get::<String>())) {
                    shell.show(&id);
                }
            }
        });
        shell.window.add_action(&open);
        for (id, button) in &shell.nav {
            let (shell, id) = (Rc::downgrade(&shell), *id);
            button.connect_clicked(move |_| {
                if let Some(shell) = shell.upgrade() {
                    shell.show(id);
                }
            });
        }
        shell.search.connect_search_changed({
            let shell = Rc::downgrade(&shell);
            move |_| {
                if let Some(shell) = shell.upgrade() {
                    shell.searched();
                }
            }
        });
        shell.search.connect_activate({
            let shell = Rc::downgrade(&shell);
            move |_| {
                if let Some(shell) = shell.upgrade()
                    && let Some(id) = *shell.first.borrow()
                {
                    shell.show(id);
                }
            }
        });
        shell.search.connect_stop_search(|search| search.set_text(""));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let shell = Rc::downgrade(&shell);
            move |_, key, _, modifiers| {
                let Some(shell) = shell.upgrade() else { return glib::Propagation::Proceed };
                if key == gdk::Key::f && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                    shell.search.grab_focus();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::w && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                    shell.window.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        shell.window.add_controller(keys);
        shell
    }

    /// Opens a pane, building it the first time.
    pub fn show(&self, id: &str) {
        let Some(pane) = panes::find(id) else { return };
        if self.pages.child_by_name(pane.id).is_none() {
            self.pages.add_named(&panes::build(pane.id), Some(pane.id));
        }
        self.pages.set_visible_child_name(pane.id);
        for (other, button) in &self.nav {
            if *other == pane.id {
                button.add_css_class("current");
            } else {
                button.remove_css_class("current");
            }
        }
        if !self.search.text().is_empty() {
            self.search.set_text("");
        }
    }

    fn searched(self: &Rc<Self>) {
        let query = self.search.text().trim().to_lowercase();
        ui::clear(&self.results);
        *self.first.borrow_mut() = None;
        self.browse.set_visible(query.is_empty());
        self.results.set_visible(!query.is_empty());
        if query.is_empty() {
            return;
        }
        let found: Vec<(&str, &str)> = INDEX.iter().filter(|(label, _)| label.to_lowercase().contains(&query)).copied().collect();
        self.list(&found, found.is_empty().then_some("No settings match."));
    }

    fn list(self: &Rc<Self>, found: &[(&'static str, &'static str)], note: Option<&str>) {
        *self.first.borrow_mut() = found.first().map(|(_, pane)| *pane);
        for (index, (label, pane)) in found.iter().enumerate() {
            let content = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let name = gtk::Label::builder().label(*label).xalign(0.0).build();
            content.append(&name);
            let where_ = gtk::Label::builder().label(panes::find(pane).map_or("", |pane| pane.label)).xalign(0.0).build();
            where_.add_css_class("pane-name");
            content.append(&where_);
            let button = gtk::Button::builder().child(&content).build();
            button.add_css_class("result");
            if index == 0 {
                button.add_css_class("first");
            }
            let (shell, pane) = (Rc::downgrade(self), *pane);
            button.connect_clicked(move |_| {
                if let Some(shell) = shell.upgrade() {
                    shell.show(pane);
                }
            });
            self.results.append(&button);
        }
        if let Some(note) = note {
            let label = gtk::Label::builder().label(note).xalign(0.0).wrap(true).build();
            label.add_css_class("search-note");
            self.results.append(&label);
        }
    }
}

fn nav_button(pane: &'static panes::Pane) -> gtk::Button {
    let tile = gtk::DrawingArea::builder().content_width(24).content_height(24).valign(gtk::Align::Center).build();
    tile.set_draw_func(move |_, cr, width, height| {
        let (width, height) = (f64::from(width), f64::from(height));
        rounded(cr, 0.0, 0.0, width, height, 6.0);
        let gradient = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, height);
        let (top, bottom) = (rgba(pane.tile.0), rgba(pane.tile.1));
        gradient.add_color_stop_rgb(0.0, top.red().into(), top.green().into(), top.blue().into());
        gradient.add_color_stop_rgb(1.0, bottom.red().into(), bottom.green().into(), bottom.blue().into());
        cr.set_source(&gradient).ok();
        cr.fill().ok();
        // The inset highlight along the top.
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
        cr.rectangle(3.0, 0.0, width - 6.0, 1.0);
        cr.fill().ok();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.translate((width - 14.0) / 2.0, (height - 14.0) / 2.0);
        icon::stroke(cr, pane.glyph, 14.0);
    });
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&tile);
    content.append(&gtk::Label::builder().label(pane.label).xalign(0.0).hexpand(true).build());
    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("nav");
    button
}

fn rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK)
}

pub fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, FRAC_PI_2, PI);
    cr.arc(x + radius, y + radius, radius, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}
