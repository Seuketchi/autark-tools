//! The player's window: the picture, with a bar of controls over it that fades while a video plays and the
//! pointer is still. Space plays and pauses, ← and → skip 5 seconds, ↑ and ↓ change the volume, M mutes, F or a
//! double click goes full screen, Ctrl+O opens a file, and a file dropped on the window plays.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};

use crate::media::{self, Event};
use autark_common::slider::Slider;

/// How long the controls stay once the pointer stops, while a video plays.
const IDLE: Duration = Duration::from_millis(2500);
const SKIP_MS: i64 = 5000;

pub struct Player {
    app: gtk::Application,
    pub window: gtk::ApplicationWindow,
    picture: gtk::Picture,
    empty: gtk::Box,
    problem: gtk::Label,
    top: gtk::Box,
    title: gtk::Label,
    controls: gtk::Box,
    play: gtk::Button,
    position: gtk::Label,
    length: gtk::Label,
    seek: Rc<Slider>,
    mute: gtk::Button,
    volume: Rc<Slider>,
    fullscreen: gtk::Button,
    media: RefCell<Option<media::Player>>,
    playing: Cell<bool>,
    ended: Cell<bool>,
    muted: Cell<bool>,
    /// Ticks since the seek bar was last moved by playback.
    ticks: Cell<u32>,
    /// Bumped each time the controls are shown, so a hide timer from before knows it's out of date.
    shown: Rc<Cell<u32>>,
    /// The cookie that keeps the screen on while a video plays.
    inhibit: Cell<u32>,
}

/// `83000` as `1:23`, and an hour or more as `1:02:03`.
fn clock(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("control");
    button
}

impl Player {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder().application(app).title("Player").default_width(1100).default_height(680).build();
        window.add_css_class("player");
        // The window's controls are drawn over the picture, so the titlebar is empty.
        window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).hexpand(true).vexpand(true).build();
        picture.set_visible(false);

        let open = gtk::Button::with_label("Open a file");
        open.add_css_class("open");
        open.set_action_name(Some("win.open"));
        let hint = gtk::Label::new(Some("Video or audio. You can also drop a file here."));
        hint.add_css_class("hint");
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 14);
        empty.set_halign(gtk::Align::Center);
        empty.set_valign(gtk::Align::Center);
        empty.append(&gtk::Image::builder().icon_name("video-x-generic-symbolic").pixel_size(56).css_classes(["glyph"]).build());
        empty.append(&open);
        empty.append(&hint);

        let problem = gtk::Label::builder().wrap(true).max_width_chars(60).justify(gtk::Justification::Center).halign(gtk::Align::Center).valign(gtk::Align::Center).visible(false).build();
        problem.add_css_class("problem");

        let title = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Middle).build();
        title.add_css_class("title");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        top.add_css_class("top");
        top.set_valign(gtk::Align::Start);
        top.append(&title);
        top.append(&autark_common::chrome::controls(true));
        top.set_visible(true);

        let play = icon_button("media-playback-start-symbolic", "Play (Space)");
        play.add_css_class("primary");
        let position = gtk::Label::new(Some("0:00"));
        position.add_css_class("time");
        let length = gtk::Label::new(Some("0:00"));
        length.add_css_class("time");
        let seek = Slider::new(0.0);
        seek.widget.set_hexpand(true);
        let mute = icon_button("audio-volume-high-symbolic", "Mute (M)");
        let volume = Slider::new(1.0);
        volume.widget.set_size_request(84, 20);
        let fullscreen = icon_button("view-fullscreen-symbolic", "Full screen (F)");
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.add_css_class("controls");
        controls.set_valign(gtk::Align::End);
        for widget in [play.upcast_ref::<gtk::Widget>(), position.upcast_ref(), seek.widget.upcast_ref(), length.upcast_ref(), mute.upcast_ref(), volume.widget.upcast_ref(), fullscreen.upcast_ref()] {
            controls.append(widget);
        }

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&empty);
        overlay.add_overlay(&problem);
        overlay.add_overlay(&top);
        overlay.add_overlay(&controls);
        window.set_child(Some(&overlay));

        let player = Rc::new(Self {
            app: app.clone(),
            window,
            picture,
            empty,
            problem,
            top,
            title,
            controls,
            play,
            position,
            length,
            seek,
            mute,
            volume,
            fullscreen,
            media: RefCell::default(),
            playing: Cell::new(false),
            ended: Cell::new(false),
            muted: Cell::new(false),
            ticks: Cell::new(0),
            shown: Rc::default(),
            inhibit: Cell::new(0),
        });
        player.connect();
        player
    }

    fn connect(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.play.connect_clicked({
            let me = me.clone();
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.toggle();
                }
            }
        });
        self.mute.connect_clicked({
            let me = me.clone();
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.toggle_mute();
                }
            }
        });
        self.fullscreen.connect_clicked({
            let me = me.clone();
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.toggle_fullscreen();
                }
            }
        });
        self.volume.connect_changed({
            let me = me.clone();
            move |value| {
                if let Some(me) = me.upgrade() {
                    me.muted.set(false);
                    me.apply_volume(value);
                }
            }
        });
        self.seek.connect_changed({
            let me = me.clone();
            move |value| {
                if let Some(me) = me.upgrade() {
                    me.seek_to_fraction(value);
                }
            }
        });

        // Ctrl+O, and the window's other actions.
        let open = gio::SimpleAction::new("open", None);
        open.connect_activate({
            let me = me.clone();
            move |_, _| {
                if let Some(me) = me.upgrade() {
                    me.choose_file();
                }
            }
        });
        self.window.add_action(&open);
        self.app.set_accels_for_action("win.open", &["<Control>o"]);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let me = me.clone();
            move |_, key, _, _| {
                let Some(me) = me.upgrade() else { return glib::Propagation::Proceed };
                me.show_controls();
                match key {
                    gdk::Key::space | gdk::Key::k => me.toggle(),
                    gdk::Key::Left => me.skip(-SKIP_MS),
                    gdk::Key::Right => me.skip(SKIP_MS),
                    gdk::Key::Up => me.nudge_volume(0.05),
                    gdk::Key::Down => me.nudge_volume(-0.05),
                    gdk::Key::m => me.toggle_mute(),
                    gdk::Key::f | gdk::Key::F11 => me.toggle_fullscreen(),
                    gdk::Key::Escape if me.window.is_fullscreen() => me.toggle_fullscreen(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        self.window.add_controller(keys);

        // A click on the picture plays or pauses; a double click goes full screen.
        let click = gtk::GestureClick::new();
        click.connect_released({
            let me = me.clone();
            move |gesture, presses, _, _| {
                let Some(me) = me.upgrade() else { return };
                if presses == 2 {
                    me.toggle_fullscreen();
                } else if presses == 1 {
                    me.toggle();
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.picture.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let me = me.clone();
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.show_controls();
                }
            }
        });
        self.window.add_controller(motion);

        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        drop.connect_drop({
            let me = me.clone();
            move |_, value, _, _| match (me.upgrade(), value.get::<gio::File>()) {
                (Some(me), Ok(file)) => {
                    me.open(&file.uri());
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(drop);

        // Frames, events and the seek bar, about 60 times a second; the window is still alive while this holds.
        glib::timeout_add_local(Duration::from_millis(16), {
            let me = me.clone();
            move || match me.upgrade() {
                Some(me) => {
                    me.tick();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });

        // The timer and the window keep each other alive until the window closes.
        let keep = self.clone();
        self.window.connect_close_request(move |_| {
            keep.close();
            glib::Propagation::Proceed
        });
    }

    /// Plays `uri` from the start.
    pub fn open(self: &Rc<Self>, uri: &str) {
        self.close();
        self.problem.set_visible(false);
        match media::Player::open(uri) {
            Ok(mut player) => {
                player.set_volume(self.volume.value(), self.muted.get());
                self.media.replace(Some(player));
                self.empty.set_visible(false);
                let name = gio::File::for_uri(uri).basename().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| uri.to_owned());
                self.title.set_text(&name);
                self.window.set_title(Some(&name));
                self.seek.set_value(0.0);
                self.position.set_text("0:00");
                self.length.set_text("0:00");
                self.play(true);
            }
            Err(error) => self.fail(&error),
        }
    }

    fn fail(&self, error: &str) {
        self.empty.set_visible(self.media.borrow().is_none());
        self.problem.set_text(&format!("Couldn’t play this: {error}"));
        self.problem.set_visible(true);
        self.show_controls();
    }

    /// Stops and lets go of what's open.
    fn close(&self) {
        self.media.replace(None);
        self.set_playing(false);
        self.picture.set_visible(false);
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.ended.set(false);
    }

    fn choose_file(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Open a video or audio file").build();
        let me = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let (Some(me), Ok(file)) = (me.upgrade(), result) {
                me.open(&file.uri());
            }
        });
    }

    fn toggle(self: &Rc<Self>) {
        if self.media.borrow().is_some() {
            self.play(!self.playing.get());
        } else {
            self.choose_file();
        }
    }

    /// Plays, or pauses; from the end it starts again.
    fn play(&self, playing: bool) {
        if playing && self.ended.replace(false) {
            if let Some(media) = self.media.borrow_mut().as_mut() {
                media.seek_ms(0);
            }
        }
        if let Some(media) = self.media.borrow_mut().as_mut() {
            media.play(playing);
        }
        self.set_playing(playing && self.media.borrow().is_some());
    }

    fn set_playing(&self, playing: bool) {
        self.playing.set(playing);
        self.play.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
        self.play.set_tooltip_text(Some(if playing { "Pause (Space)" } else { "Play (Space)" }));
        // The screen stays on while a video plays, which `autark-power` reads as an idle inhibitor.
        match (playing, self.inhibit.get()) {
            (true, 0) => self.inhibit.set(self.app.inhibit(Some(&self.window), gtk::ApplicationInhibitFlags::IDLE, Some("Playing a video"))),
            (false, cookie) if cookie != 0 => {
                self.app.uninhibit(cookie);
                self.inhibit.set(0);
            }
            _ => {}
        }
        self.show_controls();
    }

    fn skip(&self, by_ms: i64) {
        let mut media = self.media.borrow_mut();
        let Some(media) = media.as_mut() else { return };
        let now = media.position_ms().unwrap_or(0);
        let target = media.duration_ms().map_or(now + by_ms, |length| (now + by_ms).min(length));
        media.seek_ms(target.max(0));
        self.ended.set(false);
    }

    fn seek_to_fraction(&self, fraction: f64) {
        let mut media = self.media.borrow_mut();
        let Some(media) = media.as_mut() else { return };
        if let Some(length) = media.duration_ms() {
            media.seek_ms((length as f64 * fraction.clamp(0.0, 1.0)) as i64);
            self.ended.set(false);
        }
    }

    fn nudge_volume(&self, by: f64) {
        let volume = (self.volume.value() + by).clamp(0.0, 1.0);
        self.volume.set_value(volume);
        self.muted.set(false);
        self.apply_volume(volume);
    }

    fn toggle_mute(&self) {
        self.muted.set(!self.muted.get());
        self.apply_volume(self.volume.value());
    }

    fn apply_volume(&self, volume: f64) {
        let muted = self.muted.get();
        if let Some(media) = self.media.borrow_mut().as_mut() {
            media.set_volume(volume, muted);
        }
        let icon = if muted || volume == 0.0 {
            "audio-volume-muted-symbolic"
        } else if volume < 0.34 {
            "audio-volume-low-symbolic"
        } else if volume < 0.67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.mute.set_icon_name(icon);
    }

    fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
            self.fullscreen.set_icon_name("view-fullscreen-symbolic");
        } else {
            self.window.fullscreen();
            self.fullscreen.set_icon_name("view-restore-symbolic");
        }
    }

    /// Shows the controls, and while a video plays hides them again once the pointer has been still.
    fn show_controls(&self) {
        for widget in [self.top.upcast_ref::<gtk::Widget>(), self.controls.upcast_ref()] {
            widget.remove_css_class("faded");
            widget.set_can_target(true);
        }
        self.picture.set_cursor(None);
        let generation = self.shown.get().wrapping_add(1);
        self.shown.set(generation);
        if !self.playing.get() {
            return;
        }
        let (top, controls, picture, shown) = (self.top.downgrade(), self.controls.downgrade(), self.picture.downgrade(), self.shown.clone());
        glib::timeout_add_local_once(IDLE, move || {
            let (Some(top), Some(controls), Some(picture)) = (top.upgrade(), controls.upgrade(), picture.upgrade()) else { return };
            if shown.get() != generation {
                return;
            }
            for widget in [top.upcast_ref::<gtk::Widget>(), controls.upcast_ref()] {
                widget.add_css_class("faded");
                widget.set_can_target(false);
            }
            picture.set_cursor_from_name(Some("none"));
        });
    }

    /// One turn of the clock: what the pipeline said, the newest picture and, a few times a second, the seek bar.
    fn tick(self: &Rc<Self>) {
        let mut events = Vec::new();
        let mut texture = None;
        let mut progress = None;
        {
            let mut media = self.media.borrow_mut();
            let Some(media) = media.as_mut() else { return };
            while let Some(event) = media.poll() {
                events.push(event);
            }
            if let Some((width, height)) = media.frame() {
                texture = Some(gdk::MemoryTexture::new(width as i32, height as i32, gdk::MemoryFormat::B8g8r8x8, &glib::Bytes::from(media.pixels()), width as usize * 4));
            }
            let ticks = self.ticks.get() + 1;
            self.ticks.set(if ticks >= 15 { 0 } else { ticks });
            if ticks >= 15 || events.contains(&Event::Ready) {
                progress = Some((media.position_ms(), media.duration_ms()));
            }
        }
        if let Some(texture) = texture {
            self.picture.set_paintable(Some(&texture));
            self.picture.set_visible(true);
        }
        if let Some((position, length)) = progress {
            self.length.set_text(&length.map_or_else(|| "–".to_owned(), clock));
            if let Some(position) = position.filter(|_| !self.seek.dragging()) {
                self.position.set_text(&clock(position));
                if let Some(length) = length {
                    self.seek.set_value(position as f64 / length as f64);
                }
            }
        }
        for event in events {
            match event {
                Event::Ended => {
                    self.ended.set(true);
                    self.set_playing(false);
                    self.seek.set_value(1.0);
                }
                Event::Failed(error) => {
                    self.close();
                    self.fail(&error);
                }
                Event::Ready => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn times_read_as_a_player_shows_them() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(-5), "0:00");
        assert_eq!(clock(83_000), "1:23");
        assert_eq!(clock(3_723_000), "1:02:03");
    }
}
