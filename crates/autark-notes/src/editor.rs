//! The note editor: the title (the file name) above the text, styled as you type. Markdown
//! markers show only on the line being edited; links open with a click, checkboxes toggle with
//! one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_common::theme::{self, Palette};
use gtk::prelude::*;
use gtk::{gdk, pango};

use crate::markdown::{self, Span, Style};

/// A handler set once by the window.
pub type Callback<T> = RefCell<Option<Box<dyn Fn(T)>>>;

pub struct Editor {
    pub widget: gtk::ScrolledWindow,
    pub title: gtk::Entry,
    pub view: gtk::TextView,
    pub buffer: gtk::TextBuffer,
    spans: RefCell<Vec<Span>>,
    /// The line whose markers show, while the text has focus.
    editing: Cell<Option<usize>>,
    /// Set while a note is loaded, so loading doesn't count as an edit.
    loading: Cell<bool>,
    /// What the pointer pressed on, kept until release: the click itself moves the cursor and
    /// would reveal the markers it was aimed at.
    pressed: RefCell<Option<(Span, bool, bool)>>,
    /// What the find bar looks for, and where it was found, as character ranges.
    query: RefCell<String>,
    matches: RefCell<Vec<(usize, usize)>>,
    on_edit: Callback<()>,
    on_link: Callback<(String, bool)>,
    on_rename: Callback<String>,
}

impl Editor {
    pub fn new() -> Rc<Self> {
        let title = gtk::Entry::builder().placeholder_text("Title").build();
        title.add_css_class("title");
        let buffer = gtk::TextBuffer::new(None);
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .pixels_below_lines(6)
            .vexpand(true)
            .build();
        view.add_css_class("body");
        create_tags(&buffer);

        let article = gtk::Box::new(gtk::Orientation::Vertical, 10);
        article.add_css_class("article");
        article.append(&title);
        article.append(&view);
        let widget = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&article).build();

        let editor = Rc::new(Self {
            widget,
            title,
            view,
            buffer,
            spans: RefCell::default(),
            editing: Cell::new(None),
            loading: Cell::new(false),
            pressed: RefCell::default(),
            query: RefCell::default(),
            matches: RefCell::default(),
            on_edit: RefCell::default(),
            on_link: RefCell::default(),
            on_rename: RefCell::default(),
        });
        editor.connect();
        editor
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.buffer.connect_changed(move |_| {
            let Some(editor) = weak.upgrade() else { return };
            if editor.loading.get() {
                return;
            }
            editor.highlight();
            if let Some(on_edit) = &*editor.on_edit.borrow() {
                on_edit(());
            }
        });

        let weak = Rc::downgrade(self);
        self.buffer.connect_cursor_position_notify(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.follow_cursor();
            }
        });

        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.follow_cursor();
            }
        });
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.editing.set(None);
                editor.highlight();
            }
        });
        self.view.add_controller(focus);

        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).propagation_phase(gtk::PropagationPhase::Capture).build();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, presses, x, y| {
            let Some(editor) = weak.upgrade() else { return };
            let span = (presses == 1).then(|| editor.span_at(x, y)).flatten();
            let ctrl = gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK);
            let live = span.as_ref().is_some_and(|span| ctrl || editor.editing.get() != Some(editor.line_at(span.start)));
            *editor.pressed.borrow_mut() = span.map(|span| (span, live, ctrl));
        });
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, x, y| {
            let Some(editor) = weak.upgrade() else { return };
            let Some((span, live, ctrl)) = editor.pressed.take() else { return };
            if editor.span_at(x, y).as_ref() != Some(&span) {
                return;
            }
            match span.style {
                Style::Checkbox(checked) => editor.set_checked(span.start, !checked),
                Style::Link(target) if live => {
                    if let Some(on_link) = &*editor.on_link.borrow() {
                        on_link((target, ctrl));
                    }
                }
                _ => {}
            }
        });
        self.view.add_controller(click);

        // A pointer over a link or checkbox shows it's clickable.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(editor) = weak.upgrade() else { return };
            let clickable = editor.span_at(x, y).is_some();
            editor.view.set_cursor_from_name(Some(if clickable { "pointer" } else { "text" }));
        });
        self.view.add_controller(motion);

        let weak = Rc::downgrade(self);
        self.title.connect_activate(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.view.grab_focus();
            }
        });
        let title_focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        title_focus.connect_leave(move |_| {
            let Some(editor) = weak.upgrade() else { return };
            if let Some(on_rename) = &*editor.on_rename.borrow() {
                on_rename(editor.title.text().trim().to_owned());
            }
        });
        self.title.add_controller(title_focus);
    }

    pub fn connect_edit(&self, f: impl Fn() + 'static) {
        *self.on_edit.borrow_mut() = Some(Box::new(move |()| f()));
    }

    /// Called with a link's target, and whether Ctrl was held (open it in a new tab).
    pub fn connect_link(&self, f: impl Fn(String, bool) + 'static) {
        *self.on_link.borrow_mut() = Some(Box::new(move |(target, new_tab)| f(target, new_tab)));
    }

    /// Called when the title loses focus, with the title as typed.
    pub fn connect_rename(&self, f: impl Fn(String) + 'static) {
        *self.on_rename.borrow_mut() = Some(Box::new(f));
    }

    pub fn load(&self, title: &str, text: &str) {
        self.loading.set(true);
        self.title.set_text(title);
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(text);
        self.buffer.end_irreversible_action();
        self.buffer.place_cursor(&self.buffer.start_iter());
        self.loading.set(false);
        self.editing.set(None);
        self.highlight();
        self.widget.vadjustment().set_value(0.0);
    }

    /// Replaces the text after it changed on disk, keeping the cursor where it was.
    pub fn reload(&self, text: &str) {
        let offset = self.buffer.cursor_position();
        self.loading.set(true);
        self.buffer.set_text(text);
        self.buffer.place_cursor(&self.buffer.iter_at_offset(offset));
        self.loading.set(false);
        self.highlight();
    }

    pub fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, true).into()
    }

    pub fn highlight(&self) {
        let (start, end) = self.buffer.bounds();
        self.buffer.remove_all_tags(&start, &end);
        let text = self.buffer.text(&start, &end, true);
        let spans = markdown::spans(&text, self.editing.get());
        for span in &spans {
            let name = match span.style {
                Style::Heading(1) => "h1",
                Style::Heading(2) => "h2",
                Style::Heading(_) => "h3",
                Style::Bold => "bold",
                Style::Italic => "italic",
                Style::Code => "code",
                Style::Quote => "quote",
                Style::Link(_) => "link",
                Style::Tag => "tag",
                Style::Marker => "marker",
                Style::Hidden => "hidden",
                Style::Bullet => "bullet",
                Style::Checkbox(_) => "checkbox",
                Style::Done => "done",
            };
            let (from, to) = (self.buffer.iter_at_offset(span.start as i32), self.buffer.iter_at_offset(span.end as i32));
            self.buffer.apply_tag_by_name(name, &from, &to);
        }
        self.spans.replace(spans);
        let matches = find_all(&text, &self.query.borrow());
        for &(start, end) in &matches {
            self.buffer.apply_tag_by_name("match", &self.buffer.iter_at_offset(start as i32), &self.buffer.iter_at_offset(end as i32));
        }
        self.matches.replace(matches);
    }

    /// Marks every place `query` appears, ignoring case, and returns how many there are.
    pub fn find(&self, query: &str) -> usize {
        self.query.replace(query.to_owned());
        self.highlight();
        self.matches.borrow().len()
    }

    pub fn match_count(&self) -> usize {
        self.matches.borrow().len()
    }

    /// The first match at or after the cursor, wrapping to the first one.
    pub fn match_from_cursor(&self) -> usize {
        let cursor = self.buffer.cursor_position() as usize;
        self.matches.borrow().iter().position(|&(start, _)| start >= cursor).unwrap_or(0)
    }

    pub fn select_match(&self, index: usize) {
        let Some(&(start, end)) = self.matches.borrow().get(index) else { return };
        let (from, to) = (self.buffer.iter_at_offset(start as i32), self.buffer.iter_at_offset(end as i32));
        self.buffer.select_range(&from, &to);
        let mut from = from;
        self.view.scroll_to_iter(&mut from, 0.1, false, 0.0, 0.0);
    }

    pub fn replace_match(&self, index: usize, with: &str) {
        let Some(&(start, end)) = self.matches.borrow().get(index) else { return };
        self.replace_range(start, end, with);
    }

    /// Replaces every match, as one step to undo, and returns how many there were.
    pub fn replace_all(&self, with: &str) -> usize {
        let matches = self.matches.borrow().clone();
        self.buffer.begin_user_action();
        for &(start, end) in matches.iter().rev() {
            self.replace_range(start, end, with);
        }
        self.buffer.end_user_action();
        matches.len()
    }

    fn replace_range(&self, start: usize, end: usize, with: &str) {
        let mut from = self.buffer.iter_at_offset(start as i32);
        let mut to = self.buffer.iter_at_offset(end as i32);
        self.buffer.begin_user_action();
        self.buffer.delete(&mut from, &mut to);
        self.buffer.insert(&mut from, with);
        self.buffer.end_user_action();
    }

    fn follow_cursor(&self) {
        if !self.view.has_focus() {
            return;
        }
        let line = Some(self.line_at(self.buffer.cursor_position() as usize));
        if self.editing.replace(line) != line {
            self.highlight();
        }
    }

    fn line_at(&self, offset: usize) -> usize {
        self.buffer.iter_at_offset(offset as i32).line() as usize
    }

    /// The link or checkbox under a point in the view.
    fn span_at(&self, x: f64, y: f64) -> Option<Span> {
        let (bx, by) = self.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        // Past the end of a line the nearest character is still "under" the pointer.
        let location = self.view.iter_location(&iter);
        if bx > location.x() + location.width().max(8) {
            return None;
        }
        let offset = iter.offset() as usize;
        self.spans
            .borrow()
            .iter()
            .find(|span| matches!(span.style, Style::Link(_) | Style::Checkbox(_)) && (span.start..span.end).contains(&offset))
            .cloned()
    }

    fn set_checked(&self, start: usize, checked: bool) {
        let mut from = self.buffer.iter_at_offset(start as i32 + 1);
        let mut to = self.buffer.iter_at_offset(start as i32 + 2);
        self.buffer.begin_user_action();
        self.buffer.delete(&mut from, &mut to);
        self.buffer.insert(&mut from, if checked { "x" } else { " " });
        self.buffer.end_user_action();
    }

    /// Puts `[[]]` at the cursor, with the cursor inside.
    pub fn insert_link(&self) {
        self.buffer.insert_at_cursor("[[]]");
        let cursor = self.buffer.iter_at_offset(self.buffer.cursor_position() - 2);
        self.buffer.place_cursor(&cursor);
        self.view.grab_focus();
    }

    /// Makes the cursor's line a checklist item, or a plain line again.
    pub fn toggle_checklist(&self) {
        let line = self.buffer.iter_at_offset(self.buffer.cursor_position()).line();
        let Some(start) = self.buffer.iter_at_line(line) else { return };
        let mut end = start;
        end.forward_to_line_end();
        let text = self.buffer.text(&start, &end, true);
        let indent = text.len() - text.trim_start().len();
        let rest = &text[indent..];
        let (remove, insert) = if rest.starts_with("- [ ] ") || rest.starts_with("- [x] ") || rest.starts_with("- [X] ") {
            (6, "")
        } else if rest.starts_with("- ") {
            (2, "- [ ] ")
        } else {
            (0, "- [ ] ")
        };
        let indent_chars = text[..indent].chars().count() as i32;
        let mut from = self.buffer.iter_at_offset(start.offset() + indent_chars);
        let mut to = self.buffer.iter_at_offset(start.offset() + indent_chars + remove);
        self.buffer.begin_user_action();
        self.buffer.delete(&mut from, &mut to);
        self.buffer.insert(&mut from, insert);
        self.buffer.end_user_action();
        self.view.grab_focus();
    }
}

/// Where `query` appears in `text`, ignoring case, as character ranges that don't overlap.
fn find_all(text: &str, query: &str) -> Vec<(usize, usize)> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let query: Vec<char> = query.chars().map(fold).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let text: Vec<char> = text.chars().map(fold).collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + query.len() <= text.len() {
        if text[i..i + query.len()] == query[..] {
            found.push((i, i + query.len()));
            i += query.len();
        } else {
            i += 1;
        }
    }
    found
}

fn create_tags(buffer: &gtk::TextBuffer) {
    let table = buffer.tag_table();
    let tag = |name: &str| {
        let tag = gtk::TextTag::new(Some(name));
        table.add(&tag);
        tag
    };
    for (name, scale) in [("h1", 1.6), ("h2", 1.25), ("h3", 1.1)] {
        let heading = tag(name);
        heading.set_scale(scale);
        heading.set_weight(600);
        heading.set_pixels_above_lines(8);
    }
    tag("bold").set_weight(700);
    tag("italic").set_style(pango::Style::Italic);
    let code = tag("code");
    code.set_family(Some("Geist Mono, monospace"));
    tag("quote").set_left_margin(14);
    for name in ["link", "tag"] {
        tag(name).set_weight(500);
    }
    tag("hidden").set_invisible(true);
    tag("bullet").set_weight(700);
    let checkbox = tag("checkbox");
    checkbox.set_family(Some("Geist Mono, monospace"));
    checkbox.set_weight(700);
    tag("done").set_strikethrough(true);
    // Find matches, used by the find bar.
    tag("match");
    colour_tags(buffer, &theme::look().palette);
    let buffer = buffer.downgrade();
    theme::on_change(move |look| {
        if let Some(buffer) = buffer.upgrade() {
            colour_tags(&buffer, &look.palette);
        }
    });
}

/// The tags' colours, from the theme, and again whenever it changes.
fn colour_tags(buffer: &gtk::TextBuffer, palette: &Palette) {
    let table = buffer.tag_table();
    let tag = |name: &str| table.lookup(name);
    let text_alpha = |alpha: f64| {
        let [r, g, b] = theme::rgb(&palette.text);
        format!("rgba({:.0}, {:.0}, {:.0}, {alpha})", r * 255.0, g * 255.0, b * 255.0)
    };
    if let Some(code) = tag("code") {
        code.set_background(Some(&text_alpha(0.06)));
    }
    if let Some(quote) = tag("quote") {
        quote.set_foreground(Some(&palette.text2));
    }
    for name in ["link", "tag"] {
        if let Some(chip) = tag(name) {
            chip.set_foreground(Some(&palette.accent_text));
            chip.set_background(Some(&palette.accent_soft));
        }
    }
    for (name, colour) in [("marker", &palette.faint), ("bullet", &palette.accent), ("checkbox", &palette.accent), ("done", &palette.text3)] {
        if let Some(tag) = tag(name) {
            tag.set_foreground(Some(colour));
        }
    }
    if let Some(found) = tag("match") {
        found.set_background(Some(&theme::mix(&palette.warning, &palette.surface, 0.3)));
    }
}

#[cfg(test)]
mod tests {
    use super::find_all;

    #[test]
    fn find_ignores_case_and_counts_characters() {
        assert_eq!(find_all("Kith’s kith KITH", "kith"), [(0, 4), (7, 11), (12, 16)]);
        assert_eq!(find_all("aaaa", "aa"), [(0, 2), (2, 4)]);
        assert!(find_all("text", "").is_empty());
    }
}
