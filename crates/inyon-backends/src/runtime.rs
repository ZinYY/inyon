//! The core's event loop on tokio: one thread, a current-thread runtime and a `LocalSet`, as
//! Bun's single event loop is for the TS hub.
//!
//! A panic in one job ends that job only: each job runs as a tokio task of its own, and tokio
//! catches a task's panic, the way Bun turns a throw in one request handler into a 500 and goes
//! on. That needs unwinding, so the workspace's release profile unwinds; under `panic = "abort"`
//! (the Windows builds, which have no hub) any panic ends the whole process.

use std::future::Future;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::LocalBoxFuture;
use inyon_core::platform::rt::{install, Runtime};
use tokio::sync::{mpsc, oneshot};

/// [`Runtime`] on tokio: tasks go to the thread's `LocalSet`, timers to tokio's, and the clock is
/// the wall clock (`Date.now()`).
pub struct TokioRuntime;

impl Runtime for TokioRuntime {
    fn spawn(&self, fut: LocalBoxFuture<'static, ()>) {
        tokio::task::spawn_local(fut);
    }
    fn sleep(&self, ms: u64) -> LocalBoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(Duration::from_millis(ms)))
    }
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
}

/// Runs `main` as the core on this thread until it ends: a current-thread runtime with IO, time
/// and child processes, a `LocalSet` for the core's tasks, and [`TokioRuntime`] installed. Tasks
/// still running when `main` returns are dropped with the runtime, and blocking calls still in
/// flight (an outgoing request can take minutes) are left to end with the process rather than
/// waited for: when the hub is told to stop, it saves and goes, as the Bun hub's process.exit did.
pub fn run_core<F: Future>(main: impl FnOnce() -> F) -> F::Output {
    let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    let local = tokio::task::LocalSet::new();
    let out = local.block_on(&tokio, async move {
        install(Rc::new(TokioRuntime));
        main().await
    });
    drop(local);
    tokio.shutdown_background();
    out
}

type Job = Box<dyn FnOnce() -> LocalBoxFuture<'static, ()> + Send>;

/// Work for the core from other threads. Jobs start on the core's thread in the order they were
/// sent, each as a task of its own, as Bun runs request handlers.
#[derive(Clone)]
pub struct CoreHandle {
    tx: mpsc::UnboundedSender<Job>,
}

/// The job never finished: the core's thread has ended, or the job panicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreGone;

impl std::fmt::Display for CoreGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the hub's core stopped before the job finished")
    }
}

impl std::error::Error for CoreGone {}

impl CoreHandle {
    /// Runs `job` on the core's thread and gives back what it returns. The job itself (a closure)
    /// is sent across threads; the future it makes, and everything it touches, stays on the core's.
    pub fn call<R, Fut>(&self, job: impl FnOnce() -> Fut + Send + 'static) -> impl Future<Output = Result<R, CoreGone>> + Send + 'static
    where
        R: Send + 'static,
        Fut: Future<Output = R> + 'static,
    {
        let (done, answer) = oneshot::channel();
        let sent = self
            .tx
            .send(Box::new(move || {
                Box::pin(async move {
                    let _ = done.send(job().await);
                }) as LocalBoxFuture<'static, ()>
            }))
            .is_ok();
        async move {
            if !sent {
                return Err(CoreGone);
            }
            answer.await.map_err(|_| CoreGone)
        }
    }

    /// Starts `job` on the core's thread without waiting for it.
    pub fn send<Fut: Future<Output = ()> + 'static>(&self, job: impl FnOnce() -> Fut + Send + 'static) -> Result<(), CoreGone> {
        self.tx.send(Box::new(move || Box::pin(async move { job().await }) as LocalBoxFuture<'static, ()>)).map_err(|_| CoreGone)
    }
}

/// A handle for the core running on this thread (inside [`run_core`]): its jobs start here, each
/// a task of its own, for as long as a handle is kept. For a server that runs on the core's own
/// runtime (the hub), so requests never wait on a hop to another thread.
pub fn core_here() -> CoreHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    tokio::task::spawn_local(async move {
        while let Some(job) = rx.recv().await {
            tokio::task::spawn_local(job());
        }
    });
    CoreHandle { tx }
}

/// Starts the core's thread: `init` runs there first (plug in the store and the backends, start the
/// registry), then jobs from the returned handle until every handle is dropped.
pub fn core_thread(init: impl FnOnce() + Send + 'static) -> (CoreHandle, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    let thread = std::thread::Builder::new()
        .name("inyon-core".into())
        .spawn(move || {
            run_core(|| async move {
                init();
                while let Some(job) = rx.recv().await {
                    // a task of its own, not run on this loop's stack, so a panic in it ends it alone
                    tokio::task::spawn_local(job());
                }
            })
        })
        .expect("the core's thread");
    (CoreHandle { tx }, thread)
}
