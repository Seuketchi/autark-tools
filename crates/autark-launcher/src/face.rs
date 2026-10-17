//! Kith's face in the launcher and the Assistant panel: the Visor, thinking or speaking as `autark-ai`'s
//! `State` says, and listening for a moment after each key you type to Kith. One subscription to `State`
//! serves every face.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use autark_common::visor::{State, Visor};
use futures_util::StreamExt;
use gtk::glib;
use gtk::prelude::*;

use crate::ai;

/// How long a face keeps listening after the last key.
const LISTENING: Duration = Duration::from_millis(1500);

pub struct Face {
    pub visor: Visor,
    typing: Cell<Option<glib::SourceId>>,
}

thread_local! {
    static KITH: Cell<State> = const { Cell::new(State::Idle) };
    static FACES: RefCell<Vec<Weak<Face>>> = const { RefCell::new(Vec::new()) };
    static FOLLOWING: Cell<bool> = const { Cell::new(false) };
}

impl Face {
    pub fn new(size: i32) -> Rc<Self> {
        let face = Rc::new(Self { visor: Visor::new(size), typing: Cell::new(None) });
        FACES.with_borrow_mut(|faces| faces.push(Rc::downgrade(&face)));
        follow();
        face.show();
        face
    }

    /// Listens to what's typed into `entry`.
    pub fn listen_to(self: &Rc<Self>, entry: &gtk::Entry) {
        let face = Rc::downgrade(self);
        entry.connect_changed(move |entry| {
            if let Some(face) = face.upgrade()
                && !entry.text().is_empty()
            {
                face.typed();
            }
        });
    }

    fn typed(self: &Rc<Self>) {
        if let Some(source) = self.typing.take() {
            source.remove();
        }
        let face = Rc::downgrade(self);
        self.typing.set(Some(glib::timeout_add_local_once(LISTENING, move || {
            if let Some(face) = face.upgrade() {
                face.typing.set(None);
                face.show();
            }
        })));
        self.show();
    }

    fn show(&self) {
        let kith = KITH.get();
        let typing = {
            let source = self.typing.take();
            let typing = source.is_some();
            self.typing.set(source);
            typing
        };
        self.visor.set_state(if kith == State::Idle && typing { State::Listening } else { kith });
    }
}

fn show_all() {
    FACES.with_borrow_mut(|faces| faces.retain(|face| face.strong_count() > 0));
    let faces: Vec<Rc<Face>> = FACES.with_borrow(|faces| faces.iter().filter_map(Weak::upgrade).collect());
    for face in faces {
        face.show();
    }
}

/// Follows `autark-ai`'s `State` for as long as the launcher runs, starting again if it restarts.
fn follow() {
    if FOLLOWING.replace(true) {
        return;
    }
    glib::spawn_future_local(async {
        loop {
            let followed = async {
                let proxy = ai::proxy().await?;
                let mut changes = proxy.receive_state_changed().await?;
                KITH.set(State::from_word(&proxy.state().await.unwrap_or_default()));
                show_all();
                while let Some(change) = changes.next().await {
                    KITH.set(State::from_word(&change.args()?.state));
                    show_all();
                }
                zbus::Result::Ok(())
            };
            if let Err(err) = followed.await {
                eprintln!("launcher: following Kith's state: {err}");
            }
            KITH.set(State::Idle);
            show_all();
            glib::timeout_future(Duration::from_secs(10)).await;
        }
    });
}
