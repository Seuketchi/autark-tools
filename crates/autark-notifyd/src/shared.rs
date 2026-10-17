//! A value watched once and shown by many widgets: the centre's and the desktop's.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

type Listener<T> = Box<dyn Fn(&T) -> bool>;

pub struct Shared<T> {
    inner: Rc<RefCell<(T, Vec<Listener<T>>)>>,
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: 'static> Shared<T> {
    pub fn new(value: T) -> Self {
        Self { inner: Rc::new(RefCell::new((value, Vec::new()))) }
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.borrow().0.clone()
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.inner.borrow().0)
    }

    pub fn set(&self, value: T) {
        // Listeners run without the borrow held, so they may read the value themselves.
        let mut listeners = {
            let mut inner = self.inner.borrow_mut();
            inner.0 = value;
            std::mem::take(&mut inner.1)
        };
        let inner = self.inner.borrow();
        listeners.retain(|listener| listener(&inner.0));
        drop(inner);
        let mut inner = self.inner.borrow_mut();
        listeners.append(&mut inner.1);
        inner.1 = listeners;
    }

    /// Calls `show` with the value now and on every change, for as long as `widget` exists.
    pub fn watch<W: IsA<glib::Object>>(&self, widget: &W, show: impl Fn(&W, &T) + 'static) {
        show(widget, &self.inner.borrow().0);
        let widget = widget.downgrade();
        self.inner.borrow_mut().1.push(Box::new(move |value| match widget.upgrade() {
            Some(widget) => {
                show(&widget, value);
                true
            }
            None => false,
        }));
    }
}
