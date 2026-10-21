//! Keyboard: key repeat, what Caps Lock does, the layouts (Alt+Shift switches between them), and the
//! desktop's shortcuts. All of it is Wayfire's, set live over its IPC and kept in `wayfire.ini`.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use autark_common::shortcuts;

use crate::ui;
use crate::wayfire::{self, Ini};

/// The xkb options Caps Lock can take here; any other options are left alone.
const CAPS: [(&str, &str); 3] = [("caps", "Caps Lock"), ("ctrl", "Control"), ("escape", "Escape")];
const CAPS_OPTIONS: [&str; 3] = ["ctrl:nocaps", "caps:escape", "caps:ctrl_modifier"];
/// Switches layouts once there's more than one, unless another `grp:` option already does.
const SWITCH: &str = "grp:alt_shift_toggle";
const RULES: &str = "/usr/share/X11/xkb/rules/evdev.lst";

/// The shortcuts the pane offers: what they do, the command Wayfire runs, and its name in `[command]`
/// if there isn't one yet.
const SHORTCUTS: [(&str, &str, &str); 5] = [
    ("Open the launcher", "autark-launcher", "launcher"),
    ("Screenshot of the screen", "autark-shot screen", "shot_screen"),
    ("Screenshot of an area or window", "autark-shot area", "shot_area"),
    ("Screenshot options", "autark-shot options", "shot_options"),
    ("Force quit", "autark-monitor --force-quit", "force_quit"),
];

struct Keyboard {
    status: gtk::Label,
    layouts: gtk::Box,
    note: gtk::Label,
    shortcuts: gtk::Box,
}

pub fn build() -> gtk::Widget {
    let (page, column) = ui::page();
    column.append(&ui::title("Keyboard", None));
    let status = ui::note("");
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_visible(false);
    let keyboard = Rc::new(Keyboard { status, layouts: ui::group(), note: ui::note(""), shortcuts: ui::group() });
    let ini = read();
    let value = |key: &str, default: &str| ini.get("input", key).or_else(|| wayfire::get(&format!("input/{key}"))).unwrap_or_else(|| default.to_owned());

    let typing = ui::group();
    let rate: f64 = value("kb_repeat_rate", "40").parse().unwrap_or(40.0);
    let repeat = ui::slider(10.0, 60.0, 1.0, rate, "Slow", "Fast", {
        let keyboard = keyboard.clone();
        let later = ui::Debounce::default();
        move |rate| {
            let keyboard = keyboard.clone();
            later.run(std::time::Duration::from_millis(250), move || keyboard.set(&[("input/kb_repeat_rate", (rate.round() as i64).to_string())]));
        }
    });
    typing.append(&ui::row("Key repeat rate", None, &repeat));
    // Long to short reads left to right, so the slider runs backwards through the delay.
    let delay: f64 = value("kb_repeat_delay", "400").parse().unwrap_or(400.0);
    let wait = ui::slider(150.0, 800.0, 50.0, 950.0 - delay.clamp(150.0, 800.0), "Long", "Short", {
        let keyboard = keyboard.clone();
        let later = ui::Debounce::default();
        move |position| {
            let keyboard = keyboard.clone();
            // In the slider's steps of 50 ms, which a click between them doesn't keep to.
            let delay = ((950.0 - position) / 50.0).round() as i64 * 50;
            later.run(std::time::Duration::from_millis(250), move || keyboard.set(&[("input/kb_repeat_delay", delay.to_string())]));
        }
    });
    typing.append(&ui::row("Delay until repeat", None, &wait));
    let options = value("xkb_options", "");
    let caps = ui::choice(&CAPS, caps_of(&options), {
        let keyboard = keyboard.clone();
        move |caps| keyboard.set_caps(caps)
    });
    caps.update_property(&[gtk::accessible::Property::Label("Caps Lock key")]);
    typing.append(&ui::row("Caps Lock key", None, &caps));
    column.append(&typing);

    column.append(&ui::section("Layouts", Some(&keyboard.note), &keyboard.layouts));
    keyboard.show_layouts();
    column.append(&ui::section("Shortcuts", Some(&ui::note("Kept in wayfire.ini")), &keyboard.shortcuts));
    keyboard.show_shortcuts();
    let all = ui::button("All shortcuts…", |_| {
        let _ = std::process::Command::new("autark-keys").spawn();
    });
    all.set_halign(gtk::Align::Start);
    column.append(&all);
    column.append(&keyboard.status);
    page.upcast()
}

fn read() -> Ini {
    Ini::parse(&std::fs::read_to_string(wayfire::ini_path()).unwrap_or_default())
}

/// What Caps Lock does under these xkb options.
fn caps_of(options: &str) -> &'static str {
    let options: Vec<&str> = options.split(',').map(str::trim).collect();
    if options.contains(&"caps:escape") {
        "escape"
    } else if options.contains(&"ctrl:nocaps") || options.contains(&"caps:ctrl_modifier") {
        "ctrl"
    } else {
        "caps"
    }
}

/// The xkb options with Caps Lock's set to `caps` and the others kept.
fn with_caps(options: &str, caps: &str) -> String {
    let mut kept: Vec<&str> = options.split(',').map(str::trim).filter(|option| !option.is_empty() && !CAPS_OPTIONS.contains(option)).collect();
    match caps {
        "ctrl" => kept.push("ctrl:nocaps"),
        "escape" => kept.push("caps:escape"),
        _ => {}
    }
    kept.join(",")
}

/// The xkb options with the layout switch added for more than one layout, or taken out for one.
fn with_switch(options: &str, layouts: usize) -> String {
    let mut kept: Vec<&str> = options.split(',').map(str::trim).filter(|option| !option.is_empty() && *option != SWITCH).collect();
    if layouts > 1 && !kept.iter().any(|option| option.starts_with("grp:")) {
        kept.push(SWITCH);
    }
    kept.join(",")
}

/// `xkb_layout` and `xkb_variant` as (layout, variant) pairs.
fn layouts_of(layout: &str, variant: &str) -> Vec<(String, String)> {
    let variants: Vec<&str> = variant.split(',').map(str::trim).collect();
    layout.split(',').map(str::trim).filter(|code| !code.is_empty()).enumerate().map(|(index, code)| (code.to_owned(), variants.get(index).unwrap_or(&"").to_string())).collect()
}

/// Every layout and variant xkb knows, as (layout, variant, name).
fn known_layouts() -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string(RULES).unwrap_or_default();
    let mut known = Vec::new();
    let mut section = "";
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("! ") {
            section = name.trim();
            continue;
        }
        let line = line.trim();
        let Some((code, rest)) = line.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => known.push((code.to_owned(), String::new(), rest.to_owned())),
            "variant" => {
                if let Some((layout, name)) = rest.split_once(": ") {
                    known.push((layout.to_owned(), code.to_owned(), name.to_owned()));
                }
            }
            _ => {}
        }
    }
    known
}

fn layout_name(known: &[(String, String, String)], layout: &str, variant: &str) -> String {
    known
        .iter()
        .find(|(code, var, _)| code == layout && var == variant)
        .map(|(_, _, name)| name.clone())
        .unwrap_or_else(|| if variant.is_empty() { layout.to_owned() } else { format!("{layout} ({variant})") })
}

/// A key press as a Wayfire binding, if it can be one: a modifier and a key, or a function key alone.
fn binding_of(code: u32, state: gdk::ModifierType) -> Option<String> {
    shortcuts::combo(code, state.contains(gdk::ModifierType::CONTROL_MASK), state.contains(gdk::ModifierType::ALT_MASK), state.contains(gdk::ModifierType::SHIFT_MASK), state.contains(gdk::ModifierType::SUPER_MASK))
}

fn keycaps(binding: &str) -> Vec<String> {
    shortcuts::caps(binding)
}

/// Where each shortcut's binding lives in `[command]`: the name after `command_`, found by its command.
fn command_name(ini: &Ini, command: &str, fallback: &str) -> String {
    ini.keys("command")
        .into_iter()
        .filter_map(|key| key.strip_prefix("command_").map(str::to_owned))
        .find(|name| ini.get("command", &format!("command_{name}")).as_deref() == Some(command))
        .unwrap_or_else(|| fallback.to_owned())
}

impl Keyboard {
    fn report(&self, result: Result<(), String>) {
        self.status.set_visible(result.is_err());
        if let Err(error) = result {
            self.status.set_label(&error);
        }
    }

    fn set(&self, options: &[(&str, String)]) {
        self.report(wayfire::set(options));
    }

    fn input(&self, key: &str) -> String {
        read().get("input", key).or_else(|| wayfire::get(&format!("input/{key}"))).unwrap_or_default()
    }

    fn set_caps(&self, caps: &str) {
        self.set(&[("input/xkb_options", with_caps(&self.input("xkb_options"), caps))]);
    }

    fn show_layouts(self: &Rc<Self>) {
        ui::clear(&self.layouts);
        let known = known_layouts();
        let layouts = layouts_of(&self.input("xkb_layout"), &self.input("xkb_variant"));
        let layouts = if layouts.is_empty() { vec![("us".to_owned(), String::new())] } else { layouts };
        for (index, (layout, variant)) in layouts.iter().enumerate() {
            let name = layout_name(&known, layout, variant);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            let code = gtk::Label::new(Some(layout));
            code.add_css_class("code");
            code.set_width_chars(4);
            code.set_xalign(0.0);
            line.append(&code);
            let label = gtk::Label::new(Some(&name));
            label.add_css_class("row-label");
            label.set_xalign(0.0);
            label.set_hexpand(true);
            line.append(&label);
            let keyboard = self.clone();
            let remove = ui::button("Remove", move |_| {
                let mut rest = layouts_of(&keyboard.input("xkb_layout"), &keyboard.input("xkb_variant"));
                if index < rest.len() {
                    rest.remove(index);
                }
                keyboard.set_layouts(&rest);
            });
            remove.add_css_class("danger");
            remove.set_sensitive(layouts.len() > 1);
            remove.update_property(&[gtk::accessible::Property::Label(&format!("Remove {name}"))]);
            line.append(&remove);
            self.layouts.append(&line);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        line.add_css_class("row");
        let keyboard = self.clone();
        line.append(&ui::button("Add a layout…", move |button| keyboard.pick_layout(button.root().and_downcast::<gtk::Window>().as_ref())));
        self.layouts.append(&line);
        let switch = self.input("xkb_options").split(',').map(str::trim).find(|option| option.starts_with("grp:")).map(str::to_owned);
        self.note.set_label(match (layouts.len(), switch.as_deref()) {
            (1, _) => "Add one to switch between them",
            (_, Some(SWITCH)) => "Switch with Alt+Shift",
            (_, Some(_)) => "Switch with the keys in xkb_options",
            (_, None) => "Only the first is in use",
        });
    }

    fn set_layouts(self: &Rc<Self>, layouts: &[(String, String)]) {
        let layout = layouts.iter().map(|(code, _)| code.as_str()).collect::<Vec<_>>().join(",");
        let variant = layouts.iter().map(|(_, variant)| variant.as_str()).collect::<Vec<_>>().join(",");
        let options = with_switch(&self.input("xkb_options"), layouts.len());
        self.set(&[("input/xkb_layout", layout), ("input/xkb_variant", variant), ("input/xkb_options", options)]);
        self.show_layouts();
    }

    /// A searchable list of every layout xkb knows; choosing one adds it.
    fn pick_layout(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        let window = dialog("Add a layout", parent, 420, 480);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.add_css_class("page");
        let search = gtk::SearchEntry::builder().placeholder_text("Search layouts").build();
        search.update_property(&[gtk::accessible::Property::Label("Search layouts")]);
        column.append(&search);
        let list = gtk::ListBox::new();
        list.add_css_class("choices");
        list.set_activate_on_single_click(true);
        let known = known_layouts();
        for (layout, variant, name) in &known {
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let code = gtk::Label::new(Some(layout));
            code.add_css_class("code");
            code.set_width_chars(4);
            code.set_xalign(0.0);
            line.append(&code);
            let label = gtk::Label::new(Some(name));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.append(&label);
            row.set_child(Some(&line));
            row.set_widget_name(&format!("{layout}\t{variant}"));
            row.update_property(&[gtk::accessible::Property::Label(name)]);
            list.append(&row);
        }
        let names: Rc<Vec<String>> = Rc::new(known.iter().map(|(layout, _, name)| format!("{layout} {name}").to_lowercase()).collect());
        list.set_filter_func({
            let (search, names) = (search.clone(), names.clone());
            move |row| {
                let query = search.text().to_lowercase();
                query.is_empty() || names.get(row.index() as usize).is_some_and(|name| query.split_whitespace().all(|word| name.contains(word)))
            }
        });
        search.connect_search_changed({
            let list = list.clone();
            move |_| list.invalidate_filter()
        });
        let keyboard = self.clone();
        list.connect_row_activated({
            let window = window.downgrade();
            move |_, row| {
                let name = row.widget_name();
                let Some((layout, variant)) = name.split_once('\t') else { return };
                let mut layouts = layouts_of(&keyboard.input("xkb_layout"), &keyboard.input("xkb_variant"));
                if layouts.is_empty() {
                    layouts.push(("us".to_owned(), String::new()));
                }
                if !layouts.iter().any(|(code, var)| code == layout && var == variant) {
                    layouts.push((layout.to_owned(), variant.to_owned()));
                }
                keyboard.set_layouts(&layouts);
                if let Some(window) = window.upgrade() {
                    window.close();
                }
            }
        });
        let scrolled = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        column.append(&scrolled);
        window.set_child(Some(&column));
        window.present();
        search.grab_focus();
    }

    fn show_shortcuts(self: &Rc<Self>) {
        ui::clear(&self.shortcuts);
        let ini = read();
        for (what, command, fallback) in SHORTCUTS {
            let name = command_name(&ini, command, fallback);
            let binding = ini.get("command", &format!("binding_{name}"));
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            line.add_css_class("row");
            let label = gtk::Label::new(Some(what));
            label.add_css_class("row-label");
            label.set_xalign(0.0);
            label.set_hexpand(true);
            line.append(&label);
            let keys = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            keys.set_valign(gtk::Align::Center);
            match &binding {
                Some(binding) => {
                    for cap in keycaps(binding) {
                        let key = gtk::Label::new(Some(&cap));
                        key.add_css_class("keycap");
                        keys.append(&key);
                    }
                }
                None => keys.append(&ui::note("Not set")),
            }
            line.append(&keys);
            let keyboard = self.clone();
            let change = ui::button(if binding.is_some() { "Change" } else { "Set" }, move |button| {
                keyboard.capture(button.root().and_downcast::<gtk::Window>().as_ref(), what, command, fallback);
            });
            change.update_property(&[gtk::accessible::Property::Label(&format!("Change the shortcut for {}", what.to_lowercase()))]);
            line.append(&change);
            self.shortcuts.append(&line);
        }
    }

    /// Waits for the new keys of one shortcut. The compositor's own shortcuts are held off meanwhile,
    /// so the keys of another one can be taken.
    fn capture(self: &Rc<Self>, parent: Option<&gtk::Window>, what: &'static str, command: &'static str, fallback: &'static str) {
        let window = dialog("Change shortcut", parent, 380, -1);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.add_css_class("page");
        let prompt = gtk::Label::new(Some(&format!("Press the keys for “{what}”")));
        prompt.add_css_class("row-label");
        prompt.set_wrap(true);
        column.append(&prompt);
        let hint = ui::note("A modifier and a key, or a function key. Esc cancels.");
        hint.set_wrap(true);
        column.append(&hint);
        let problem = ui::note("");
        problem.add_css_class("error");
        problem.set_visible(false);
        column.append(&problem);
        let cancel = ui::button("Cancel", {
            let window = window.downgrade();
            move |_| {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
            }
        });
        cancel.set_halign(gtk::Align::End);
        column.append(&cancel);
        window.set_child(Some(&column));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let keyboard = self.clone();
        let weak = window.downgrade();
        keys.connect_key_pressed(move |_, _, keycode, state| {
            let code = keycode.saturating_sub(8);
            let state = state & gtk::accelerator_get_default_mod_mask();
            if code == 1 && state.is_empty() {
                if let Some(window) = weak.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            let Some(binding) = binding_of(code, state) else { return glib::Propagation::Stop };
            let ini = read();
            let name = command_name(&ini, command, fallback);
            let taken = SHORTCUTS
                .iter()
                .find(|(_, other, other_fallback)| *other != command && ini.get("command", &format!("binding_{}", command_name(&ini, other, other_fallback))).as_deref() == Some(binding.as_str()));
            if let Some((other, ..)) = taken {
                problem.set_label(&format!("{} is already “{other}”", keycaps(&binding).join("+")));
                problem.set_visible(true);
                return glib::Propagation::Stop;
            }
            // Wayfire rereads `wayfire.ini` when it changes, which is how `[command]`'s bindings apply.
            keyboard.report(wayfire::write(&[(&format!("command/binding_{name}"), binding), (&format!("command/command_{name}"), command.to_owned())]));
            keyboard.show_shortcuts();
            if let Some(window) = weak.upgrade() {
                window.close();
            }
            glib::Propagation::Stop
        });
        window.add_controller(keys);
        window.connect_map(|window| {
            if let Some(toplevel) = window.surface().and_downcast::<gdk::Toplevel>() {
                toplevel.inhibit_system_shortcuts(None::<&gdk::Event>);
            }
        });
        window.connect_unmap(|window| {
            if let Some(toplevel) = window.surface().and_downcast::<gdk::Toplevel>() {
                toplevel.restore_system_shortcuts();
            }
        });
        window.present();
    }
}

/// A modal window over Settings, counted as Settings by the bar and dock.
fn dialog(title: &str, parent: Option<&gtk::Window>, width: i32, height: i32) -> gtk::Window {
    let window = gtk::Window::builder().title(title).modal(true).resizable(false).default_width(width).default_height(height).build();
    window.set_transient_for(parent);
    window.set_application(parent.and_then(|parent| parent.application()).as_ref());
    window.add_css_class("settings");
    window.add_css_class("dialog");
    window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_lock_options_swap_and_others_stay() {
        assert_eq!(caps_of("grp:alt_shift_toggle,caps:escape"), "escape");
        assert_eq!(with_caps("grp:alt_shift_toggle,caps:escape", "ctrl"), "grp:alt_shift_toggle,ctrl:nocaps");
        assert_eq!(with_caps("ctrl:nocaps", "caps"), "");
    }

    #[test]
    fn the_switch_comes_and_goes_with_layouts() {
        assert_eq!(with_switch("caps:escape", 2), "caps:escape,grp:alt_shift_toggle");
        assert_eq!(with_switch("caps:escape,grp:alt_shift_toggle", 1), "caps:escape");
        assert_eq!(with_switch("grp:win_space_toggle", 2), "grp:win_space_toggle", "someone's own switch stays");
    }

    #[test]
    fn layouts_pair_with_variants() {
        assert_eq!(layouts_of("us,de", ",nodeadkeys"), [("us".to_owned(), String::new()), ("de".to_owned(), "nodeadkeys".to_owned())]);
        assert_eq!(layouts_of("us", ""), [("us".to_owned(), String::new())]);
    }

    #[test]
    fn bindings_read_and_capture() {
        assert_eq!(keycaps("<shift> <super> KEY_3"), ["Shift", "Super", "3"]);
        assert_eq!(keycaps("<ctrl> <alt> KEY_ESC"), ["Ctrl", "Alt", "Esc"]);
        assert_eq!(binding_of(57, gdk::ModifierType::SUPER_MASK).as_deref(), Some("<super> KEY_SPACE"));
        assert_eq!(binding_of(4, gdk::ModifierType::SUPER_MASK | gdk::ModifierType::SHIFT_MASK).as_deref(), Some("<shift> <super> KEY_3"));
        assert_eq!(binding_of(30, gdk::ModifierType::empty()), None, "a plain letter would stop typing");
        assert_eq!(binding_of(30, gdk::ModifierType::SHIFT_MASK), None);
        assert_eq!(binding_of(88, gdk::ModifierType::empty()).as_deref(), Some("KEY_F12"));
    }

    #[test]
    fn shortcuts_are_found_by_command() {
        let ini = Ini::parse("[command]\nbinding_l = <super> KEY_SPACE\ncommand_l = autark-launcher\n");
        assert_eq!(command_name(&ini, "autark-launcher", "launcher"), "l");
        assert_eq!(command_name(&ini, "autark-shot area", "shot_area"), "shot_area");
    }
}
