//! The Apps view: every installed app as an icon and a name, in alphabetical order, for when you don't
//! know what to type. It takes the place of the results while it shows, and typing goes back to them.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::pango;
use gtk::prelude::*;

pub struct Grid {
    pub root: gtk::ScrolledWindow,
    flow: gtk::FlowBox,
    apps: RefCell<Vec<gio::AppInfo>>,
}

impl Grid {
    /// `open` is called with the app that was clicked or chosen with Enter.
    pub fn new(open: impl Fn(&gio::AppInfo) + 'static) -> Rc<Self> {
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .homogeneous(true)
            .min_children_per_line(4)
            .max_children_per_line(6)
            .row_spacing(4)
            .column_spacing(4)
            .valign(gtk::Align::Start)
            .build();
        flow.add_css_class("app-grid");
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            // A flow box reports the height of its narrowest layout, so without a floor the rows that wrap
            // below the first two are cut off.
            .min_content_height(340)
            .max_content_height(520)
            .propagate_natural_height(true)
            .child(&flow)
            .build();
        root.add_css_class("apps");
        root.set_visible(false);
        let grid = Rc::new(Self { root, flow, apps: RefCell::default() });
        grid.flow.connect_child_activated({
            let grid = Rc::downgrade(&grid);
            move |_, child| {
                let Some(grid) = grid.upgrade() else { return };
                let app = usize::try_from(child.index()).ok().and_then(|index| grid.apps.borrow().get(index).cloned());
                if let Some(app) = app {
                    open(&app);
                }
            }
        });
        grid
    }

    /// Shows `apps`, which the caller has sorted.
    pub fn fill(&self, apps: Vec<gio::AppInfo>) {
        self.flow.remove_all();
        for app in &apps {
            let icon = match app.icon() {
                Some(icon) => gtk::Image::from_gicon(&icon),
                None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
            };
            icon.set_pixel_size(48);
            let name = gtk::Label::builder().label(app.display_name()).max_width_chars(12).ellipsize(pango::EllipsizeMode::End).build();
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
            tile.add_css_class("app-tile");
            tile.append(&icon);
            tile.append(&name);
            tile.set_tooltip_text(app.description().as_deref().map(|text| text.to_string()).as_deref());
            self.flow.append(&tile);
        }
        *self.apps.borrow_mut() = apps;
    }

    /// Moves the keyboard to the first app, for the arrow keys to carry on from.
    pub fn focus(&self) {
        if let Some(first) = self.flow.child_at_index(0) {
            self.flow.select_child(&first);
            first.grab_focus();
        }
    }
}
