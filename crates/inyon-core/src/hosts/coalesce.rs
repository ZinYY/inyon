//! Port of hosts/coalesce.ts.

use std::cell::RefCell;
use std::rc::Rc;

use crate::platform::rt::{set_timeout, Timer};

struct State {
    timer: Option<Timer>,
    again: bool,
}

/// Runs `f` for the first call at once, and for anything that follows within `ms` once more at
/// the end of the window. A burst of changes (several machines answering at the same time, a
/// screen update on every pane) reaches each app as one message instead of one per change.
pub fn coalesce(f: impl Fn() + 'static, ms: u64) -> Rc<dyn Fn()> {
    let f: Rc<dyn Fn()> = Rc::new(f);
    let state = Rc::new(RefCell::new(State { timer: None, again: false }));
    Rc::new(move || {
        {
            let mut s = state.borrow_mut();
            if s.timer.is_some() {
                s.again = true;
                return;
            }
        }
        f();
        arm(state.clone(), f.clone(), ms);
    })
}

fn arm(state: Rc<RefCell<State>>, f: Rc<dyn Fn()>, ms: u64) {
    let st = state.clone();
    let timer = set_timeout(ms, move || {
        {
            let mut s = st.borrow_mut();
            if !s.again {
                s.timer = None;
                return;
            }
            s.again = false;
        }
        f();
        arm(st.clone(), f.clone(), ms);
    });
    state.borrow_mut().timer = Some(timer);
}

#[cfg(test)]
#[path = "coalesce_tests.rs"]
mod tests;
