//! The window: tabs of terminals, a search bar, and the confirmation bar for a paste Kith rates risky.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use vte::{Terminal, TerminalExt};

use crate::{check, tab};

/// PCRE2_CASELESS | PCRE2_UTF | PCRE2_MULTILINE, which VTE's search wants spelled out.
const SEARCH_FLAGS: u32 = 0x0000_0008 | 0x0008_0000 | 0x0000_0400;

pub struct Window {
    pub window: gtk::ApplicationWindow,
    notebook: gtk::Notebook,
    search_bar: gtk::SearchBar,
    search: gtk::SearchEntry,
    warning: gtk::Revealer,
    warning_text: gtk::Label,
    /// The paste waiting for a yes, with the terminal it was meant for.
    held: RefCell<Option<(Terminal, String)>>,
    zoom: Cell<f64>,
}

impl Window {
    pub fn new(app: &gtk::Application, folder: Option<&Path>) -> Rc<Self> {
        let notebook = gtk::Notebook::builder().scrollable(true).show_border(false).hexpand(true).vexpand(true).build();
        notebook.add_css_class("tabs");
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("flat");
        add.set_tooltip_text(Some("New tab (Ctrl+Shift+T)"));
        notebook.set_action_widget(&add, gtk::PackType::End);

        let search = gtk::SearchEntry::builder().hexpand(true).placeholder_text("Search").build();
        let search_bar = gtk::SearchBar::builder().child(&search).build();

        let warning_text = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).build();
        let paste_anyway = gtk::Button::with_label("Paste anyway");
        let cancel = gtk::Button::with_label("Cancel");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("paste-warning");
        row.append(&warning_text);
        row.append(&cancel);
        row.append(&paste_anyway);
        let warning = gtk::Revealer::builder().child(&row).transition_type(gtk::RevealerTransitionType::SlideDown).build();

        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&search_bar);
        column.append(&warning);
        column.append(&notebook);

        let window = gtk::ApplicationWindow::builder().application(app).default_width(900).default_height(560).title("Terminal").child(&column).build();
        window.add_css_class("terminal-window");

        // The design's title bar: round close, minimise and maximise on the left, the title centred.
        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        controls.set_decoration_layout(Some("close,minimize,maximize:"));
        controls.set_valign(gtk::Align::Center);
        let title = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).build();
        title.add_css_class("window-title");
        window.bind_property("title", &title, "label").sync_create().build();
        let bar = gtk::CenterBox::new();
        bar.add_css_class("terminal-titlebar");
        bar.set_start_widget(Some(&controls));
        bar.set_center_widget(Some(&title));
        window.set_titlebar(Some(&gtk::WindowHandle::builder().child(&bar).build()));

        let this = Rc::new(Self { window, notebook, search_bar, search, warning, warning_text, held: RefCell::default(), zoom: Cell::new(1.0) });

        add.connect_clicked({
            let this = this.clone();
            move |_| this.new_tab(this.current_folder().as_deref())
        });
        this.notebook.connect_page_added({
            let this = this.clone();
            move |_, _, _| this.show_tabs_if_several()
        });
        this.notebook.connect_page_removed({
            let this = this.clone();
            move |_, _, _| this.show_tabs_if_several()
        });
        this.notebook.connect_switch_page({
            let this = this.clone();
            move |_, page, _| {
                if let Some(terminal) = page.downcast_ref::<Terminal>() {
                    this.window.set_title(Some(&title_of(terminal)));
                }
            }
        });
        cancel.connect_clicked({
            let this = this.clone();
            move |_| this.drop_held()
        });
        paste_anyway.connect_clicked({
            let this = this.clone();
            move |_| this.paste_held()
        });
        this.search.connect_search_changed({
            let this = this.clone();
            move |entry| this.search_for(&entry.text())
        });
        this.search.connect_next_match({
            let this = this.clone();
            move |_| {
                if let Some(terminal) = this.current() {
                    terminal.search_find_next();
                }
            }
        });
        this.search.connect_previous_match({
            let this = this.clone();
            move |_| {
                if let Some(terminal) = this.current() {
                    terminal.search_find_previous();
                }
            }
        });
        this.search.connect_stop_search({
            let this = this.clone();
            move |_| {
                this.search_bar.set_search_mode(false);
                if let Some(terminal) = this.current() {
                    terminal.grab_focus();
                }
            }
        });
        this.shortcuts();
        this.new_tab(folder);
        this
    }

    fn current(&self) -> Option<Terminal> {
        self.notebook.nth_page(self.notebook.current_page()).and_downcast::<Terminal>()
    }

    fn current_folder(&self) -> Option<std::path::PathBuf> {
        self.current().and_then(|terminal| tab::folder(&terminal))
    }

    fn show_tabs_if_several(&self) {
        self.notebook.set_show_tabs(self.notebook.n_pages() > 1);
    }

    pub fn new_tab(self: &Rc<Self>, folder: Option<&Path>) {
        let terminal = tab::new(folder, {
            let this = self.clone();
            move |terminal| this.close_tab(terminal)
        });
        terminal.set_font_scale(self.zoom.get());

        let title = gtk::Label::builder().label("Terminal").ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(22).build();
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.add_css_class("tab-close");
        let label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        label.append(&title);
        label.append(&close);
        close.connect_clicked({
            let this = self.clone();
            let terminal = terminal.clone();
            move |_| this.close_tab(&terminal)
        });
        terminal.connect_window_title_notify({
            let this = self.clone();
            let title = title.clone();
            move |terminal| {
                let text = title_of(terminal);
                title.set_label(&text);
                if this.current().as_ref() == Some(terminal) {
                    this.window.set_title(Some(&text));
                }
            }
        });

        let page = self.notebook.append_page(&terminal, Some(&label));
        self.notebook.set_tab_reorderable(&terminal, true);
        self.notebook.set_current_page(Some(page));
        terminal.grab_focus();
    }

    fn close_tab(&self, terminal: &Terminal) {
        if let Some(page) = self.notebook.page_num(terminal) {
            self.notebook.remove_page(Some(page));
        }
        if self.notebook.n_pages() == 0 {
            self.window.close();
        } else if let Some(terminal) = self.current() {
            terminal.grab_focus();
        }
    }

    fn zoom_by(&self, factor: Option<f64>) {
        let zoom = factor.map_or(1.0, |factor| (self.zoom.get() * factor).clamp(0.5, 4.0));
        self.zoom.set(zoom);
        for page in 0..self.notebook.n_pages() {
            if let Some(terminal) = self.notebook.nth_page(Some(page)).and_downcast::<Terminal>() {
                terminal.set_font_scale(zoom);
            }
        }
    }

    fn search_for(&self, text: &str) {
        let Some(terminal) = self.current() else {
            return;
        };
        if text.is_empty() {
            terminal.search_set_regex(None, 0);
            return;
        }
        match vte::Regex::for_search(&glib::Regex::escape_string(text), SEARCH_FLAGS) {
            Ok(regex) => {
                terminal.search_set_regex(Some(&regex), 0);
                terminal.search_set_wrap_around(true);
                terminal.search_find_next();
            }
            Err(error) => eprintln!("autark-terminal: search: {error}"),
        }
    }

    /// Ctrl+Shift+V: what's on the clipboard goes to the shell, unless it would run by itself and Kith rates it risky.
    fn paste(self: &Rc<Self>) {
        let Some(terminal) = self.current() else {
            return;
        };
        let this = self.clone();
        terminal.clipboard().read_text_async(None::<&gio::Cancellable>, move |result| {
            let Ok(Some(text)) = result else { return };
            let text = text.to_string();
            if !check::runs_itself(&text) {
                terminal.paste_text(&text);
                return;
            }
            let held = text.clone();
            check::rate(&text, move |rating| match rating {
                Some(rating) if rating.risk >= check::ASK_AT => {
                    this.warning_text.set_label(&format!("This paste could do damage: {}. Paste it anyway?", rating.reason));
                    *this.held.borrow_mut() = Some((terminal, held));
                    this.warning.set_reveal_child(true);
                }
                _ => terminal.paste_text(&held),
            });
        });
    }

    fn paste_held(&self) {
        if let Some((terminal, text)) = self.held.borrow_mut().take() {
            terminal.paste_text(&text);
            terminal.grab_focus();
        }
        self.warning.set_reveal_child(false);
    }

    fn drop_held(&self) {
        self.held.borrow_mut().take();
        self.warning.set_reveal_child(false);
        if let Some(terminal) = self.current() {
            terminal.grab_focus();
        }
    }

    fn shortcuts(self: &Rc<Self>) {
        let controller = gtk::ShortcutController::new();
        // Capture, so VTE doesn't take the keys first.
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let add = |keys: &str, action: Box<dyn Fn() + 'static>| {
            if let Some(trigger) = gtk::ShortcutTrigger::parse_string(keys) {
                let action = gtk::CallbackAction::new(move |_, _| {
                    action();
                    glib::Propagation::Stop
                });
                controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
            }
        };
        let this = self.clone();
        add(
            "<Control><Shift>t",
            Box::new({
                let this = this.clone();
                move || this.new_tab(this.current_folder().as_deref())
            }),
        );
        add(
            "<Control><Shift>w",
            Box::new({
                let this = this.clone();
                move || {
                    if let Some(terminal) = this.current() {
                        this.close_tab(&terminal);
                    }
                }
            }),
        );
        add(
            "<Control><Shift>n",
            Box::new({
                let this = this.clone();
                move || {
                    if let Some(app) = this.window.application() {
                        Window::new(&app, this.current_folder().as_deref()).window.present();
                    }
                }
            }),
        );
        add(
            "<Control><Shift>c",
            Box::new({
                let this = this.clone();
                move || {
                    if let Some(terminal) = this.current() {
                        terminal.copy_clipboard_format(vte::Format::Text);
                    }
                }
            }),
        );
        add(
            "<Control><Shift>v",
            Box::new({
                let this = this.clone();
                move || this.paste()
            }),
        );
        add(
            "<Control><Shift>f",
            Box::new({
                let this = this.clone();
                move || {
                    this.search_bar.set_search_mode(true);
                    this.search.grab_focus();
                }
            }),
        );
        for keys in ["<Control>plus", "<Control>equal", "<Control>KP_Add"] {
            add(
                keys,
                Box::new({
                    let this = this.clone();
                    move || this.zoom_by(Some(1.1))
                }),
            );
        }
        for keys in ["<Control>minus", "<Control>KP_Subtract"] {
            add(
                keys,
                Box::new({
                    let this = this.clone();
                    move || this.zoom_by(Some(1.0 / 1.1))
                }),
            );
        }
        for keys in ["<Control>0", "<Control>KP_0"] {
            add(
                keys,
                Box::new({
                    let this = this.clone();
                    move || this.zoom_by(None)
                }),
            );
        }
        add(
            "<Control>Page_Down",
            Box::new({
                let this = this.clone();
                move || this.notebook.next_page()
            }),
        );
        add(
            "<Control>Page_Up",
            Box::new({
                let this = this.clone();
                move || this.notebook.prev_page()
            }),
        );
        for page in 1..=9u32 {
            add(
                &format!("<Alt>{page}"),
                Box::new({
                    let this = this.clone();
                    move || {
                        if page - 1 < this.notebook.n_pages() {
                            this.notebook.set_current_page(Some(page - 1));
                        }
                    }
                }),
            );
        }
        self.window.add_controller(controller);
    }
}

/// What a terminal's title is for the tab: the program's, or "Terminal".
fn title_of(terminal: &Terminal) -> String {
    terminal.window_title().map(|title| title.to_string()).filter(|title| !title.is_empty()).unwrap_or_else(|| "Terminal".to_owned())
}

// `gdk` is for the clipboard's types when the bindings need them spelled out.
#[allow(unused_imports)]
use gdk as _;
