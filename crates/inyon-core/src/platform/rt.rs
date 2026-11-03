//! The core's event loop, as Bun's is for the TS hub: one thread, tasks that run until they
//! await, timers, and promises. The platform installs a [`Runtime`] on the thread the core lives
//! on (tokio's LocalSet in the hub, the browser's event loop on the phone); everything here runs
//! on that thread, so state needs no locks and a synchronous stretch of code is never interleaved
//! with another task, which is what the TS relies on.
//!
//! - [`spawn`]: calling an async function without awaiting it: it runs at once up to its first
//!   await that has to wait, then goes on as a task of its own.
//! - [`sleep`]: `await new Promise((r) => setTimeout(r, ms))`.
//! - [`set_timeout`] / [`Timer::clear`]: `setTimeout` / `clearTimeout`.
//! - [`Deferred`]: a promise many tasks can await, resolved once.
//! - [`spawn_task`]: a promise for a task's result (`const p = (async () => …)()`).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use futures_util::future::{self, Either, LocalBoxFuture};

/// What a platform gives the core: somewhere to run tasks, a timer and a clock.
pub trait Runtime {
    /// Runs `fut` on the core's thread until it ends; never on another thread.
    fn spawn(&self, fut: LocalBoxFuture<'static, ()>);
    /// A future that's ready `ms` milliseconds from now.
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'static, ()>;
    /// `Date.now()`
    fn now_ms(&self) -> u64;
}

thread_local! {
    static RT: RefCell<Option<Rc<dyn Runtime>>> = const { RefCell::new(None) };
}

/// Makes `rt` this thread's event loop, and its clock the one [`crate::clock::now_ms`] reads.
pub fn install(rt: Rc<dyn Runtime>) {
    let clock = rt.clone();
    crate::clock::set_source(Some(Rc::new(move || clock.now_ms())));
    RT.with(|r| *r.borrow_mut() = Some(rt));
}

/// Whether this thread runs the stateful core (a runtime was installed on it). The registry, the
/// watchers, the drafts and the store are this thread's own; on any other thread they'd be empty
/// copies that nothing else sees.
pub fn on_core_thread() -> bool {
    RT.with(|r| r.borrow().is_some())
}

/// Debug builds stop at the first use of the core's state off its thread: such a call would work
/// on an empty registry (or store) of its own and quietly do nothing the hub sees.
#[track_caller]
pub(crate) fn assert_core_thread(what: &str) {
    debug_assert!(on_core_thread(), "{what} used on a thread without the core's runtime (platform::rt::install); hand the work to the core's thread instead");
}

fn runtime() -> Rc<dyn Runtime> {
    RT.with(|r| r.borrow().clone()).expect("inyon-core used on a thread without a runtime (platform::rt::install)")
}

/// Calls an async function without awaiting it (`void f()`): like JavaScript, it runs right away,
/// on the caller's stack, up to the first await that has to wait; the rest goes on as a task.
///
/// The first step is polled with a waker that goes nowhere. That's sound because the task polls
/// the future again as soon as it starts, and every future registers the waker it's polled with
/// then (a Deferred, a timer and a channel all do).
pub fn spawn(fut: impl Future<Output = ()> + 'static) {
    let mut fut: LocalBoxFuture<'static, ()> = Box::pin(fut);
    let waker = futures_util::task::noop_waker();
    if fut.as_mut().poll(&mut Context::from_waker(&waker)).is_ready() {
        return;
    }
    runtime().spawn(fut);
}

/// `await Promise.resolve()`: lets every task that's ready run first, then goes on. The nearest a
/// task queue comes to a microtask.
pub fn yield_now() -> impl Future<Output = ()> {
    let mut yielded = false;
    future::poll_fn(move |cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
}

/// `await sleep(ms)`
pub fn sleep(ms: u64) -> LocalBoxFuture<'static, ()> {
    runtime().sleep(ms)
}

struct TimerState {
    cancelled: Cell<bool>,
    fired: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

/// A pending `setTimeout`. Dropping it leaves the timer running, as forgetting a timeout's id does.
#[derive(Clone)]
pub struct Timer(Rc<TimerState>);

impl Timer {
    /// `clearTimeout`: the callback won't run (a no-op once it has).
    pub fn clear(&self) {
        self.0.cancelled.set(true);
        if let Some(w) = self.0.waker.borrow_mut().take() {
            w.wake();
        }
    }

    /// Neither run nor cleared yet.
    pub fn pending(&self) -> bool {
        !self.0.cancelled.get() && !self.0.fired.get()
    }
}

struct Cancelled(Rc<TimerState>);

impl Future for Cancelled {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.cancelled.get() {
            return Poll::Ready(());
        }
        *self.0.waker.borrow_mut() = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// `setTimeout(f, ms)`. `f` runs as a task's synchronous step; async work it starts goes through
/// [`spawn`].
pub fn set_timeout(ms: u64, f: impl FnOnce() + 'static) -> Timer {
    let state = Rc::new(TimerState { cancelled: Cell::new(false), fired: Cell::new(false), waker: RefCell::new(None) });
    let timer = Timer(state.clone());
    let wait = sleep(ms);
    spawn(async move {
        if let Either::Left(_) = future::select(wait, Cancelled(state.clone())).await {
            if !state.cancelled.get() {
                state.fired.set(true);
                f();
            }
        }
    });
    timer
}

/// `clearTimeout(t)` for a timer kept in an `Option`, which is left empty.
pub fn clear(t: &mut Option<Timer>) {
    if let Some(t) = t.take() {
        t.clear();
    }
}

struct DeferredState<T> {
    value: Option<T>,
    wakers: Vec<Waker>,
}

/// A promise: resolved once, awaited by any number of tasks (each gets a clone of the value).
pub struct Deferred<T>(Rc<RefCell<DeferredState<T>>>);

impl<T> Clone for Deferred<T> {
    fn clone(&self) -> Self {
        Deferred(self.0.clone())
    }
}

impl<T: Clone + 'static> Default for Deferred<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone + 'static> Deferred<T> {
    pub fn new() -> Self {
        Deferred(Rc::new(RefCell::new(DeferredState { value: None, wakers: vec![] })))
    }

    /// `Promise.resolve(v)`
    pub fn resolved(v: T) -> Self {
        let d = Self::new();
        d.resolve(v);
        d
    }

    /// Settles it; the first value wins, as a promise's does.
    pub fn resolve(&self, v: T) {
        let wakers = {
            let mut s = self.0.borrow_mut();
            if s.value.is_some() {
                return;
            }
            s.value = Some(v);
            std::mem::take(&mut s.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }

    /// The value, once settled.
    pub fn value(&self) -> Option<T> {
        self.0.borrow().value.clone()
    }

    pub fn is_settled(&self) -> bool {
        self.0.borrow().value.is_some()
    }

    /// `await p`
    pub fn wait(&self) -> impl Future<Output = T> + 'static {
        let d = self.clone();
        future::poll_fn(move |cx| {
            let mut s = d.0.borrow_mut();
            match &s.value {
                Some(v) => Poll::Ready(v.clone()),
                None => {
                    s.wakers.push(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
    }

    /// Two handles to the same promise.
    pub fn same(&self, other: &Deferred<T>) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Starts `fut` now as a task of its own and returns the promise of its result: it runs to the end
/// whether or not anyone awaits it, as an async function's promise does.
pub fn spawn_task<T: Clone + 'static>(fut: impl Future<Output = T> + 'static) -> Deferred<T> {
    let d = Deferred::new();
    let out = d.clone();
    spawn(async move { out.resolve(fut.await) });
    d
}
