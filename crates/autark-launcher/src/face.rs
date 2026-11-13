//! Kith's face in the launcher: the Visor, thinking or speaking as `autark-ai`'s `State` says. One subscription
//! to `State` serves every face.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use autark_common::visor::{State, Visor};
use futures_util::StreamExt;
use gtk::glib;

use crate::ai;

pub struct Face {
    pub visor: Visor,
}

thread_local! {
    static KITH: Cell<State> = const { Cell::new(State::Idle) };
    static FACES: RefCell<Vec<Weak<Face>>> = const { RefCell::new(Vec::new()) };
    static FOLLOWING: Cell<bool> = const { Cell::new(false) };
}

impl Face {
    pub fn new(size: i32) -> Rc<Self> {
        let face = Rc::new(Self { visor: Visor::new(size) });
        FACES.with_borrow_mut(|faces| faces.push(Rc::downgrade(&face)));
        follow();
        face.show();
        face
    }

    fn show(&self) {
        self.visor.set_state(KITH.get());
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
